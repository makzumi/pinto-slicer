//! Printer profiles, support settings and print settings.
//!
//! A profile owns the exact LCD resolution and the native job format. Build
//! dimensions are deliberately not used to infer a rounded resolution: several
//! printers have fractional pixel pitches and odd panel widths.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JobFormat {
    CxdlpV4,
    Cxdlp,
    Ctb,
    Goo,
    Photon,
    Pwmo,
    Pwmx,
    Pm3,
    Pws,
    Pwma,
    Sl1,
    Sl1s,
}

impl JobFormat {
    pub const ALL: [Self; 12] = [
        Self::CxdlpV4,
        Self::Cxdlp,
        Self::Ctb,
        Self::Goo,
        Self::Photon,
        Self::Pwmo,
        Self::Pwmx,
        Self::Pm3,
        Self::Pws,
        Self::Pwma,
        Self::Sl1,
        Self::Sl1s,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Self::CxdlpV4 => "cxdlpv4",
            Self::Cxdlp => "cxdlp",
            Self::Ctb => "ctb",
            Self::Goo => "goo",
            Self::Photon => "photon",
            Self::Pwmo => "pwmo",
            Self::Pwmx => "pwmx",
            Self::Pm3 => "pm3",
            Self::Pws => "pws",
            Self::Pwma => "pwma",
            Self::Sl1 => "sl1",
            Self::Sl1s => "sl1s",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CxdlpV4 => "Creality CXDLP v4",
            Self::Cxdlp => "Creality CXDLP v3",
            Self::Ctb => "ChiTuBox CTB",
            Self::Goo => "ELEGOO GOO",
            Self::Photon => "ChiTuBox Photon",
            Self::Pwmo => "Anycubic Photon Mono (PWMO)",
            Self::Pwmx => "Anycubic Photon Mono X (PWMX)",
            Self::Pm3 => "Anycubic Photon M3 (PM3)",
            Self::Pws => "Anycubic Photon / Photon S (PWS)",
            Self::Pwma => "Anycubic Photon Mono 4K (PWMA)",
            Self::Sl1 => "Prusa SL1 archive",
            Self::Sl1s => "Prusa SL1S archive",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        Self::ALL.into_iter().find(|f| f.extension() == ext)
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PrinterProfile {
    /// Friendly name shown in the printer picker.
    pub name: String,
    /// Model string written into printer files (often different from the UI name).
    pub machine_name: String,
    /// Build volume in millimetres: width (X), depth (Y), height (Z).
    pub build: [f32; 3],
    /// Exact native LCD resolution in pixels.
    pub resolution: [u32; 2],
    /// Native file written by the Export step.
    pub format: JobFormat,
    /// Formats accepted by this machine. The first/native one is automatic;
    /// alternatives such as Photon/PWS remain profile-constrained.
    pub formats: Vec<JobFormat>,
    /// LCD mirroring required by this printer's optical path.
    pub mirror: [bool; 2],
    pub note: String,
}

impl PrinterProfile {
    pub fn native_res(&self) -> (usize, usize) {
        (self.resolution[0] as usize, self.resolution[1] as usize)
    }

    pub fn pixel_mm(&self) -> [f32; 2] {
        [
            self.build[0] / self.resolution[0].max(1) as f32,
            self.build[1] / self.resolution[1].max(1) as f32,
        ]
    }
    pub fn supports_format(&self, format: JobFormat) -> bool {
        self.formats.contains(&format)
    }
}

fn profile(
    name: &str,
    machine_name: &str,
    build: [f32; 3],
    resolution: [u32; 2],
    format: JobFormat,
    mirror: [bool; 2],
) -> PrinterProfile {
    PrinterProfile {
        name: name.into(),
        machine_name: machine_name.into(),
        build,
        resolution,
        format,
        formats: vec![format],
        mirror,
        note: format!(
            "{} · native {} × {} px · .{}",
            format.label(),
            resolution[0],
            resolution[1],
            format.extension()
        ),
    }
}

/// Ten common machines covering the principal Creality, ChiTu, ELEGOO,
/// Anycubic and Prusa families. Dimensions and panel data follow UVtools'
/// machine registry; files are still expected to be checked on the target
/// printer before a long production print.
pub fn default_printers() -> Vec<PrinterProfile> {
    let mut printers = vec![
        profile(
            "Creality HALOT-X1",
            "HALOT-X1",
            [211.68, 118.37, 200.0],
            [15120, 6230],
            JobFormat::CxdlpV4,
            [false, false],
        ),
        profile(
            "Creality HALOT-ONE",
            "CL-60",
            [81.0, 128.0, 160.0],
            [1620, 2560],
            JobFormat::Cxdlp,
            [false, false],
        ),
        profile(
            "ELEGOO Saturn 3 Ultra",
            "Saturn 3 Ultra",
            [218.88, 122.88, 260.0],
            [11520, 5120],
            JobFormat::Ctb,
            [true, false],
        ),
        profile(
            "ELEGOO Saturn 4 Ultra 12K",
            "Saturn 4 Ultra 12K",
            [218.88, 122.88, 220.0],
            [11520, 5120],
            JobFormat::Goo,
            [true, false],
        ),
        profile(
            "Anycubic Photon",
            "Photon",
            [68.04, 120.96, 155.0],
            [1440, 2560],
            JobFormat::Photon,
            [true, false],
        ),
        profile(
            "Anycubic Photon Mono",
            "Photon Mono",
            [82.62, 130.56, 165.0],
            [1620, 2560],
            JobFormat::Pwmo,
            [true, false],
        ),
        profile(
            "Anycubic Photon Mono X",
            "Photon Mono X",
            [192.0, 120.0, 245.0],
            [3840, 2400],
            JobFormat::Pwmx,
            [true, false],
        ),
        profile(
            "Anycubic Photon M3",
            "Photon M3",
            [163.84, 102.4, 180.0],
            [4096, 2560],
            JobFormat::Pm3,
            [true, false],
        ),
        profile(
            "Original Prusa SL1",
            "SL1",
            [68.04, 120.96, 150.0],
            [1440, 2560],
            JobFormat::Sl1,
            [true, false],
        ),
        profile(
            "Original Prusa SL1S SPEED",
            "SL1S",
            [81.0, 128.0, 150.0],
            [1620, 2560],
            JobFormat::Sl1s,
            [true, false],
        ),
    ];
    printers[4].formats = vec![JobFormat::Photon, JobFormat::Pws];
    printers
}

/// Hardware profiles available to batch export. This includes the ten
/// featured GUI profiles plus less common writers that still have distinct
/// panel requirements and therefore should not be disguised as aliases.
pub fn export_printers() -> Vec<PrinterProfile> {
    let mut printers = default_printers();
    printers.push(profile(
        "Anycubic Photon Mono 4K",
        "Photon Mono 4K",
        [134.4, 84.0, 165.0],
        [3840, 2400],
        JobFormat::Pwma,
        [true, false],
    ));
    printers
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PrintSettings {
    pub layer_height: f32,
    pub exposure: f32,
    pub bottom_layers: u32,
    pub bottom_exposure: f32,
    /// How far the platform rises between layers, in millimetres.
    pub rising_height: f32,
    /// Platform travel speed, in millimetres per minute.
    pub motor_speed: f32,
    /// Rest after the platform settles and before the next exposure, in seconds.
    pub light_off_delay: f32,
    pub supports: crate::supports::SupportConfig,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            layer_height: 0.05,
            exposure: 2.5,
            bottom_layers: 5,
            bottom_exposure: 35.0,
            rising_height: 6.0,
            motor_speed: 240.0,
            light_off_delay: 0.5,
            supports: crate::supports::SupportConfig::default(),
        }
    }
}

impl PrintSettings {
    /// Time spent moving the platform up and back down for one layer. The
    /// platform travels the rise twice (up, then back down) at `motor_speed`.
    pub fn travel_seconds(&self) -> f32 {
        (2.0 * self.rising_height.max(0.0) / self.motor_speed.max(1.0)) * 60.0
    }

    /// Seconds one layer takes from exposure to exposure.
    pub fn layer_seconds(&self, index: usize) -> f32 {
        let exposure = if (index as u32) < self.bottom_layers {
            self.bottom_exposure
        } else {
            self.exposure
        };
        exposure + self.travel_seconds() + self.light_off_delay.max(0.0)
    }

    /// Rough print time: exposure, lift/retract, and the light-off settle.
    pub fn estimated_minutes(&self, layers: usize) -> f32 {
        let secs: f32 = (0..layers).map(|i| self.layer_seconds(i)).sum();
        secs / 60.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_halot_profile_uses_the_real_native_panel() {
        let p = &default_printers()[0];
        assert_eq!(p.native_res(), (15120, 6230));
        assert_eq!(p.format, JobFormat::CxdlpV4);
        assert!((p.pixel_mm()[0] - 0.014).abs() < 0.00001);
    }

    #[test]
    fn ten_profiles_have_valid_and_unique_native_output() {
        let printers = default_printers();
        assert_eq!(printers.len(), 10);
        for p in printers {
            assert!(p.native_res().0 >= 64 && p.native_res().1 >= 64);
            assert!(JobFormat::from_extension(p.format.extension()) == Some(p.format));
            assert!(p.supports_format(p.format));
            assert!(p.build.iter().all(|v| *v > 0.0));
        }
    }

    #[test]
    fn prusa_models_keep_their_own_panels_and_mirror_x() {
        let printers = default_printers();
        assert_eq!(printers[8].native_res(), (1440, 2560));
        assert_eq!(printers[9].native_res(), (1620, 2560));
        assert!(printers[8].mirror[0] && printers[9].mirror[0]);
        assert_ne!(printers[8].build, printers[9].build);
        assert_eq!(printers[9].machine_name, "SL1S");
    }

    #[test]
    fn every_requested_extension_is_recognised() {
        for ext in [
            "ctb", "goo", "cxdlp", "cxdlpv4", "photon", "pwmo", "pwmx", "pm3", "pws", "pwma",
            "sl1", "sl1s",
        ] {
            assert!(JobFormat::from_extension(ext).is_some(), "{ext}");
        }
    }

    #[test]
    fn presets_get_heavier() {
        let light = crate::supports::SupportConfig::from_preset(crate::supports::Preset::Light);
        let heavy = crate::supports::SupportConfig::from_preset(crate::supports::Preset::Heavy);
        assert!(heavy.trunk_diameter > light.trunk_diameter);
        assert!(heavy.tip.lower_diameter > light.tip.lower_diameter);
        // The preset is the strut weight; how many there are is the density row.
        assert_eq!(heavy.touch_tip_distance, light.touch_tip_distance);
    }
}
