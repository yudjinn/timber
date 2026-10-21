//! Node categories for colouring the graph.

use timber_core::{NodeId, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Root,
    Device,
    Cpu,
    Bus,
    Clock,
    Interrupt,
    Power,
    Pinctrl,
    Memory,
    /// Nodes without `compatible`: configuration, endpoints, pin groups' children.
    Data,
}

impl Kind {
    /// Legend order. Categorical colours are assigned in this fixed order.
    pub const ALL: [Kind; 10] = [
        Kind::Device,
        Kind::Cpu,
        Kind::Bus,
        Kind::Clock,
        Kind::Interrupt,
        Kind::Power,
        Kind::Pinctrl,
        Kind::Memory,
        Kind::Data,
        Kind::Root,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Root => "Root",
            Kind::Device => "Device",
            Kind::Cpu => "CPU",
            Kind::Bus => "Bus",
            Kind::Clock => "Clock",
            Kind::Interrupt => "Interrupt controller",
            Kind::Power => "Power / regulator",
            Kind::Pinctrl => "GPIO / pinctrl",
            Kind::Memory => "Memory",
            Kind::Data => "Data / config",
        }
    }

    /// Fill colour. The eight hues are the dataviz reference categorical palette, validated
    /// for CVD separation in both modes; root and data nodes stay neutral.
    pub fn color(self, dark: bool) -> &'static str {
        match (self, dark) {
            (Kind::Device, false) => "#2a78d6",
            (Kind::Device, true) => "#3987e5",
            (Kind::Cpu, false) => "#eb6834",
            (Kind::Cpu, true) => "#d95926",
            (Kind::Bus, false) => "#1baf7a",
            (Kind::Bus, true) => "#199e70",
            (Kind::Clock, false) => "#eda100",
            (Kind::Clock, true) => "#c98500",
            (Kind::Interrupt, false) => "#e87ba4",
            (Kind::Interrupt, true) => "#d55181",
            (Kind::Power, _) => "#008300",
            (Kind::Pinctrl, false) => "#6250d6",
            (Kind::Pinctrl, true) => "#9085e9",
            (Kind::Memory, false) => "#e34948",
            (Kind::Memory, true) => "#e66767",
            (Kind::Data, false) => "#a9a8a2",
            (Kind::Data, true) => "#6e6d68",
            (Kind::Root, false) => "#3d3c39",
            (Kind::Root, true) => "#d6d5d0",
        }
    }
}

pub fn classify(tree: &Tree, id: NodeId) -> Kind {
    if id == Tree::ROOT {
        return Kind::Root;
    }
    let node = tree.node(id);
    let base = node.base_name();
    let device_type = tree.prop_str(id, "device_type");
    let compatible = tree.prop_strings(id, "compatible");
    let has = |p: &str| node.has_prop(p);
    let parent_name = node.parent.map(|p| tree.node(p).base_name()).unwrap_or("");
    // Pin groups and configs: no compatible, somewhere under a pinctrl node. Checked first
    // so names like `pmic` or `clk` inside pinctrl don't masquerade as devices.
    let in_pinctrl = || {
        tree.ancestors(id)
            .iter()
            .any(|&a| tree.node(a).base_name().contains("pinctrl"))
    };
    if compatible.is_empty() && !has("gpio-controller") && in_pinctrl() {
        return Kind::Pinctrl;
    }

    if device_type == Some("cpu")
        || matches!(base, "cpus" | "cpu" | "cpu-map")
        || parent_name == "cpu-map"
    {
        Kind::Cpu
    } else if device_type == Some("memory")
        || matches!(base, "memory" | "reserved-memory")
        || parent_name == "reserved-memory"
    {
        Kind::Memory
    } else if has("gpio-controller") || base.contains("pinctrl") || base.contains("pinmux") {
        // GPIO banks are often interrupt controllers too; GPIO is the more useful label.
        Kind::Pinctrl
    } else if has("interrupt-controller") || has("msi-controller") {
        Kind::Interrupt
    } else if has("#clock-cells") {
        Kind::Clock
    } else if has("regulator-name")
        || base.starts_with("regulator")
        || base.contains("pmic")
        || has("#power-domain-cells")
        || compatible.iter().any(|c| c.contains("regulator"))
    {
        Kind::Power
    } else if compatible
        .iter()
        .any(|c| matches!(*c, "simple-bus" | "simple-mfd" | "arm,amba-bus"))
        || matches!(base, "soc" | "bus")
    {
        Kind::Bus
    } else if !compatible.is_empty() {
        Kind::Device
    } else {
        Kind::Data
    }
}
