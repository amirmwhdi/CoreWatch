//! Graphics cards on the PCI bus, and their marketing names.

use std::path::Path;

/// PCI IDs as the kernel prints them: `0x1002` becomes `1002`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PciIds {
    pub vendor: String,
    pub device: String,
    pub subsystem_vendor: String,
    pub subsystem_device: String,
}

impl PciIds {
    pub fn read(device_dir: &Path) -> Self {
        let id = |file: &str| {
            std::fs::read_to_string(device_dir.join(file))
                .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
                .unwrap_or_default()
        };
        Self {
            vendor: id("vendor"),
            device: id("device"),
            subsystem_vendor: id("subsystem_vendor"),
            subsystem_device: id("subsystem_device"),
        }
    }
}

/// Where distributions keep the PCI ID database (hwdata or pciutils), and
/// the same files on the host as seen from Flatpak.
pub const PCI_IDS_PATHS: [&str; 5] = [
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/usr/share/pci.ids",
    "/run/host/usr/share/hwdata/pci.ids",
    "/run/host/usr/share/misc/pci.ids",
];

/// The model name in `pci.ids` text: the card maker's own name for the
/// board when it is a model name ("Radeon RX 580 Armor 8G OC"), else the
/// name in brackets ("GeForce RTX 3060"), else the chip's name.
///
/// Vendors start a line, devices are indented by one tab, boards (subsystem
/// vendor and device) by two:
///
/// ```text
/// 1002  Advanced Micro Devices, Inc. [AMD/ATI]
///     67df  Ellesmere [Radeon RX 470/480/570/570X/580/580X/590]
///         1462 3417  Radeon RX 580 Armor 4G OC
/// ```
pub fn model_name(pci_ids: &str, ids: &PciIds) -> Option<String> {
    let mut lines = pci_ids.lines();
    // The vendor's block.
    lines.by_ref().find(|l| {
        l.split_whitespace().next() == Some(ids.vendor.as_str()) && !l.starts_with('\t')
    })?;
    let mut device_name = None;
    let mut subsystem_name = None;
    for line in lines {
        if !line.starts_with('\t') {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            break; // next vendor
        }
        if let Some(sub) = line.strip_prefix("\t\t") {
            if device_name.is_none() {
                continue;
            }
            let mut words = sub.splitn(3, |c: char| c.is_whitespace());
            let (sv, sd) = (words.next(), words.next());
            if sv == Some(ids.subsystem_vendor.as_str())
                && sd == Some(ids.subsystem_device.as_str())
            {
                subsystem_name = words.next().map(|n| n.trim().to_owned());
            }
        } else if device_name.is_some() {
            break; // next device
        } else if let Some((id, name)) = line[1..].split_once(|c: char| c.is_whitespace()) {
            if id == ids.device {
                device_name = Some(name.trim().to_owned());
            }
        }
    }
    let device_name = device_name?;
    let bracket = device_name.rfind('[').and_then(|start| {
        device_name[start + 1..]
            .strip_suffix(']')
            .map(str::to_owned)
    });
    // A subsystem name is often a laptop model ("ThinkPad T14"); take it only
    // when it names the same kind of card as the bracket ("Radeon ...").
    let family = bracket
        .as_deref()
        .and_then(|b| b.split_whitespace().next())
        .map(str::to_owned);
    let subsystem = subsystem_name.filter(|s| family.as_deref().is_some_and(|f| s.contains(f)));
    Some(subsystem.or(bracket).unwrap_or(device_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: &str = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t67df  Ellesmere [Radeon RX 470/480/570/570X/580/580X/590]
\t\t1462 3417  Radeon RX 580 Armor 4G OC
\t\t1028 1234  Some OEM Board
\t67ef  Baffin [Radeon RX 460/560D / Pro 450/455/460/555/555X/560/560X]
10de  NVIDIA Corporation
\t2504  GA106 [GeForce RTX 3060 Lite Hash Rate]
\t\t1462 397d  RTX 3060 Ventus 2X 12G
8086  Intel Corporation
\t3e9b  CoffeeLake-H GT2 [UHD Graphics 630]
\t\t17aa 2266  ThinkPad P1
\t9a49  TigerLake-LP GT2 [Iris Xe Graphics]
";

    fn ids(v: &str, d: &str, sv: &str, sd: &str) -> PciIds {
        PciIds {
            vendor: v.into(),
            device: d.into(),
            subsystem_vendor: sv.into(),
            subsystem_device: sd.into(),
        }
    }

    #[test]
    fn names_come_from_the_board_or_the_bracket() {
        let name = |i| model_name(IDS, &i);
        assert_eq!(
            name(ids("1002", "67df", "1462", "3417")).as_deref(),
            Some("Radeon RX 580 Armor 4G OC")
        );
        // An OEM board name that is not a card name: the bracket.
        assert_eq!(
            name(ids("1002", "67df", "1028", "1234")).as_deref(),
            Some("Radeon RX 470/480/570/570X/580/580X/590")
        );
        assert_eq!(
            name(ids("10de", "2504", "1462", "397d")).as_deref(),
            Some("GeForce RTX 3060 Lite Hash Rate")
        );
        assert_eq!(
            name(ids("8086", "3e9b", "17aa", "2266")).as_deref(),
            Some("UHD Graphics 630")
        );
        assert_eq!(name(ids("8086", "ffff", "", "")), None);
    }
}
