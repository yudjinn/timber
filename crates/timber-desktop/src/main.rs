//! Timber desktop: the Leptos UI in a native window, with native file dialogs, folders read
//! straight from disk, and folder watching. SSH (M5) will live here too.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod files;
mod watch;

use std::path::PathBuf;

use serde::Serialize;
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// What was passed on the command line:
/// `timber [FILE…] [--source DIR…] [--config FILE] [--smoke | --smoke-watch]`.
#[derive(Debug, Clone, Default, Serialize)]
struct Startup {
    files: Vec<String>,
    sources: Vec<String>,
    /// A kernel `.config`/defconfig or Yocto conf (`--config`).
    config: Option<String>,
    /// Load the arguments, print a one-line summary, and exit (used to test the app).
    smoke: bool,
    /// Like `smoke`, but after the first summary wait for a watched folder to change and
    /// reload, then print a second summary and exit.
    smoke_watch: bool,
}

fn parse_args() -> Startup {
    let mut s = Startup::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source" | "-s" => s.sources.extend(args.next()),
            "--config" | "-c" => s.config = args.next(),
            "--smoke" => s.smoke = true,
            "--smoke-watch" => s.smoke_watch = true,
            path => s.files.push(path.to_string()),
        }
    }
    let absolute =
        |p: String| std::fs::canonicalize(&p).map_or(p, |a| a.to_string_lossy().into_owned());
    s.files = s.files.into_iter().map(absolute).collect();
    s.sources = s.sources.into_iter().map(absolute).collect();
    s.config = s.config.map(absolute);
    s
}

const CANCELLED: &[u8] = &[];

#[tauri::command]
fn startup(state: State<'_, Startup>) -> Startup {
    state.inner().clone()
}

/// Prompts for files and returns them packed; empty if cancelled.
#[tauri::command]
async fn pick_files(app: AppHandle, any: Option<bool>) -> Result<Response, String> {
    let dialog = app.dialog().file();
    // `any`: one file of any type (a kernel config or Yocto conf).
    let picked = if any.unwrap_or(false) {
        dialog
            .set_title("Add a kernel config or Yocto conf")
            .blocking_pick_file()
            .map(|f| vec![f])
    } else {
        dialog
            .set_title("Open device tree files")
            .add_filter(
                "Device trees and sources",
                &["dtb", "dtbo", "dts", "dtsi", "dtso", "h"],
            )
            .add_filter("All files", &["*"])
            .blocking_pick_files()
    };
    let Some(picked) = picked else {
        return Ok(Response::new(CANCELLED.to_vec()));
    };
    let paths: Vec<PathBuf> = picked
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    Ok(Response::new(files::pack("", &files::read_files(&paths)?)))
}

/// Reads files given by path (command-line arguments).
#[tauri::command]
async fn read_paths(paths: Vec<String>) -> Result<Response, String> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    Ok(Response::new(files::pack("", &files::read_files(&paths)?)))
}

/// Prompts for a folder and returns its source files packed, with the folder path as the
/// handle; empty if cancelled.
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Response, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("Add a source folder")
        .blocking_pick_folder();
    let Some(dir) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(Response::new(CANCELLED.to_vec()));
    };
    read_folder(dir.to_string_lossy().into_owned()).await
}

/// Reads a folder's source files (to add it, or to reload it).
#[tauri::command]
async fn read_folder(path: String) -> Result<Response, String> {
    let files = files::read_folder(std::path::Path::new(&path))?;
    Ok(Response::new(files::pack(&path, &files)))
}

/// Saves bytes sent as the raw request body; the suggested name is in `x-file-name`.
/// Returns false if the user cancelled.
#[tauri::command]
async fn save_file(app: AppHandle, request: Request<'_>) -> Result<bool, String> {
    let InvokeBody::Raw(data) = request.body() else {
        return Err("expected raw file data".into());
    };
    let name = request
        .headers()
        .get("x-file-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("export")
        .to_string();
    let Some(path) = app
        .dialog()
        .file()
        .set_file_name(&name)
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// Splits a command template into arguments: spaces separate, double quotes group.
fn split_command(template: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in template.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    args.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        args.push(cur);
    }
    args
}

/// Opens a source file in the user's editor: the Settings template (docs/EDITORS.md) with
/// `{file}`, `{line}`, `{col}` substituted per argument, or the OS default application.
#[tauri::command]
fn open_in_editor(path: String, line: u32, col: u32, template: String) -> Result<(), String> {
    let args: Vec<String> = if template.trim().is_empty() {
        #[cfg(target_os = "linux")]
        let opener = vec!["xdg-open".to_string(), path.clone()];
        #[cfg(target_os = "macos")]
        let opener = vec!["open".to_string(), path.clone()];
        #[cfg(target_os = "windows")]
        let opener = vec![
            "cmd".into(),
            "/C".into(),
            "start".into(),
            String::new(),
            path.clone(),
        ];
        opener
    } else {
        split_command(&template)
            .into_iter()
            .map(|a| {
                a.replace("{file}", &path)
                    .replace("{line}", &line.to_string())
                    .replace("{col}", &col.to_string())
            })
            .collect()
    };
    let (program, rest) = args.split_first().ok_or("the editor command is empty")?;
    std::process::Command::new(program)
        .args(rest)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("couldn't run `{program}`: {e}"))
}

/// Prints a line to stdout (the `--smoke` run's report).
#[tauri::command]
fn report(line: String) {
    println!("{line}");
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(parse_args())
        .setup(|app| {
            app.manage(watch::Watchers::new(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            startup,
            pick_files,
            read_paths,
            pick_folder,
            read_folder,
            save_file,
            watch::watch_folder,
            watch::unwatch_folder,
            open_in_editor,
            report,
            quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Timber");
}

#[cfg(test)]
mod tests {
    use super::split_command;

    #[test]
    fn splits_templates() {
        assert_eq!(
            split_command("code -g {file}:{line}:{col}"),
            ["code", "-g", "{file}:{line}:{col}"]
        );
        assert_eq!(
            split_command(r#""C:\Program Files\x.exe" -n{line}  {file}"#),
            [r"C:\Program Files\x.exe", "-n{line}", "{file}"]
        );
        assert_eq!(split_command("kitty -e nvim +{line} {file}").len(), 5);
    }
}
