# Security

Timber handles files people often can't share: unreleased board designs, vendor BSPs, and SSH access to devices. Reports of anything that could expose them are very welcome.

**Please report vulnerabilities privately** through GitHub: the repository's **Security** tab → **Report a vulnerability**. Don't open a public issue for them.

## What Timber promises

- **Files stay on your computer.** The web build reads files inside the page and never uploads them. Its Content-Security-Policy only lets the page connect to its own origin (where the app itself is served from), so the browser blocks any attempt to send data elsewhere (`tests/ui/specs/csp.spec.js` checks this).
- **The desktop app** reads files from disk and connects only to the devices you tell it to, over SSH. Passwords and passphrases are never stored. Host keys you accept go to Timber's own `known_hosts`; `~/.ssh/known_hosts` is read but never changed.
- **No telemetry, accounts or servers** of Timber's own. The web build is static files.
