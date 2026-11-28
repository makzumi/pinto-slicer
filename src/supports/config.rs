//! Support settings — the numbers the panel exposes and the generator reads.
//!
//! Defaults follow the reference "Column Support – Medium" panel.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TipShape {
    /// A cone that narrows to the model.
    Cone,
    /// A straight pin.
    Cylinder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContactShape {
    /// No extra shape: the tip itself is pressed into the surface.
    None,
    /// A small ball at the very end.
    Ball,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TipConfig {
    pub shape: TipShape,
    /// Touches the model.
    pub upper_diameter: f32,
    /// Away from the model, where the branch joins.
    pub lower_diameter: f32,
    pub length: f32,
    pub contact_shape: ContactShape,
    pub contact_diameter: f32,
    /// The tip is pressed this far into the surface.
    pub contact_depth: f32,
}

impl Default for TipConfig {
    fn default() -> Self {
        Self {
            shape: TipShape::Cone,
            upper_diameter: 0.50,
            lower_diameter: 0.75,
            length: 2.5,
            contact_shape: ContactShape::None,
            contact_diameter: 0.8,
            contact_depth: 0.4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaseConfig {
    /// Add a base plate under the feet at all.
    pub enabled: bool,
    pub plate_thickness: f32,
    pub margin: f32,
    pub chamfer_deg: f32,
    /// One monolithic sheet per connected group of feet instead of splitting
    /// into many small capped pads. Left in the file format for older
    /// projects, but no longer exposed in the panel.
    #[serde(default)]
    pub raft: bool,
}

impl Default for BaseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            plate_thickness: 0.85,
            margin: 2.5,
            chamfer_deg: 15.0,
            raft: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    Light,
    Medium,
    Heavy,
}

impl Preset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Medium => "Medium",
            Self::Heavy => "Heavy",
        }
    }
}
/// Three spacings between tips, for the panel's density row.
///
/// These are distances the generator actually aims for: `touch_tip_distance`
/// is how far the coverage circles reach, so a wider value is a sparser
/// network, not just a bigger tip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Low,
    Medium,
    High,
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// Target millimetres between tips on overhang areas.
    pub fn spacing(self) -> f32 {
        match self {
            Self::Low => 8.0,
            Self::Medium => 6.0,
            Self::High => 4.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SupportConfig {
    /// Which preset the numbers came from, for the panel's highlight.
    pub preset: Preset,
    /// The model is lifted this far above the platform before anything is
    /// generated, so the support network has room.
    pub height_from_platform: f32,
    /// Target spacing between tips on overhang areas. This is the distance
    /// the coverage radius is derived from: bigger means fewer, further-apart
    /// tips, so it is the density control as well as a spacing one.
    pub touch_tip_distance: f32,
    /// Measured from vertical: a face is flagged when normal.z < -sin(angle).
    /// With `overhangs_only` off, every face that points down is flagged
    /// instead and the angle only matters for the debug view.
    pub overhang_angle_deg: f32,
    /// HALOT Box calls the same switch "Overhangs Only". Off (the proven
    /// profile): shallow slopes and flat undersides get support too. On:
    /// only faces steeper than `overhang_angle_deg`.
    pub overhangs_only: bool,
    pub tip: TipConfig,
    pub branch_angle_deg: f32,
    pub branch_diameter: f32,
    /// Trunk width where the tips meet. A trunk carrying many tips grows
    /// half again wider ([`SupportConfig::trunk_diameter_for`]).
    pub trunk_diameter: f32,
    /// Trunk width at the foot: a trunk that stands on the platform is a
    /// cone, this wide at the bottom and slim where the model meets it —
    /// the shape of the columns the machine already prints well.
    pub trunk_diameter_max: f32,
    pub foot_diameter: f32,
    pub base: BaseConfig,
    /// Grow supports at all (the panel switch).
    pub enabled: bool,
}

impl Default for SupportConfig {
    fn default() -> Self {
        Self {
            preset: Preset::Medium,
            height_from_platform: 6.0,
            // The spacing every well-printed HALOT Box file has used: tips
            // 4 mm apart, which this app calls the High density.
            touch_tip_distance: Density::High.spacing(),
            overhang_angle_deg: 45.0,
            overhangs_only: true,
            tip: TipConfig::default(),
            branch_angle_deg: 45.0,
            branch_diameter: 0.85,
            trunk_diameter: 0.85,
            // HALOT Box's columns are cones: ~2.5 mm at the foot, slimming
            // to the shaft width where the model meets it.
            trunk_diameter_max: 2.5,
            foot_diameter: 1.5,
            base: BaseConfig::default(),
            enabled: true,
        }
    }
}

impl SupportConfig {
    /// A preset sizes the network: tips, branches, trunks and feet. It leaves
    /// the tip spacing to the density row — one button quietly rewriting the
    /// other's number is what made the two rows look broken.
    pub fn from_preset(p: Preset) -> Self {
        let mut c = Self::default();
        c.apply_preset(p);
        c
    }

    /// Fills in a preset's numbers but keeps the on/off switch and the density.
    pub fn apply_preset(&mut self, p: Preset) {
        // Start from the default sizes, so going Heavy -> Light cannot keep the
        // heavier numbers that Light does not list.
        let d = Self::default();
        self.tip.upper_diameter = d.tip.upper_diameter;
        self.tip.lower_diameter = d.tip.lower_diameter;
        self.branch_diameter = d.branch_diameter;
        self.trunk_diameter = d.trunk_diameter;
        self.trunk_diameter_max = d.trunk_diameter_max;
        self.foot_diameter = d.foot_diameter;
        match p {
            Preset::Light => {
                self.tip.lower_diameter = 0.7;
                self.tip.upper_diameter = 0.55;
                self.branch_diameter = 0.7;
                self.trunk_diameter = 0.7;
                self.trunk_diameter_max = 1.8;
                self.foot_diameter = 1.8;
            }
            Preset::Medium => {}
            Preset::Heavy => {
                self.tip.lower_diameter = 1.1;
                self.tip.upper_diameter = 0.8;
                self.branch_diameter = 1.0;
                self.trunk_diameter = 1.1;
                self.trunk_diameter_max = 3.4;
                self.foot_diameter = 3.4;
            }
        }
        self.preset = p;
    }

    /// Closest two tips may be.
    pub fn min_tip_spacing(&self) -> f32 {
        self.touch_tip_distance.max(0.4)
    }

    /// No overhang area may be further than this from a tip.
    pub fn max_unsupported_radius(&self) -> f32 {
        self.touch_tip_distance * 1.5
    }

    /// Tips are clustered this far apart before routing.
    pub fn cluster_radius(&self) -> f32 {
        self.touch_tip_distance * 2.5
    }

    /// How deep a support that stands on the model is pressed into it. A foot
    /// resting on material makes the same bond a tip does, so it is the same
    /// depth; asking for it in one place keeps the two from drifting apart.
    pub fn landing_depth(&self) -> f32 {
        self.tip.contact_depth.clamp(0.05, 1.0)
    }

    /// Narrower than this and the printed branch would not hold.
    pub fn tip_radius(&self) -> f32 {
        (self.tip.lower_diameter * 0.5).max(0.1)
    }

    /// Trunk width for a trunk carrying `tips` tips: 0.85 -> 1.275 mm where
    /// the tips meet. The foot of a standing trunk is `trunk_diameter_max`
    /// wide instead.
    pub fn trunk_diameter_for(&self, tips: usize) -> f32 {
        let t = ((tips as f32 - 1.0) / 6.0).clamp(0.0, 1.0);
        self.trunk_diameter * (1.0 + 0.5 * t)
    }

    /// A hand-written JSON form, so settings can be saved without a JSON crate.
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{\n",
                "  \"height_from_platform\": {},\n",
                "  \"touch_tip_distance\": {},\n",
                "  \"overhang_angle_deg\": {},\n",
                "  \"tip\": {{ \"shape\": \"{:?}\", \"upper_diameter\": {}, \"lower_diameter\": {}, ",
                "\"length\": {}, \"contact_shape\": \"{:?}\", \"contact_diameter\": {}, \"contact_depth\": {} }},\n",
                "  \"branch_angle_deg\": {},\n",
                "  \"branch_diameter\": {},\n",
                "  \"trunk_diameter\": {},\n",
                "  \"trunk_diameter_max\": {},\n",
                "  \"foot_diameter\": {},\n",
                "  \"base\": {{ \"enabled\": {}, \"plate_thickness\": {}, \"margin\": {}, \"chamfer_deg\": {}, \"raft\": {} }},\n",
                "  \"enabled\": {}\n",
                "}}"
            ),
            self.height_from_platform,
            self.touch_tip_distance,
            self.overhang_angle_deg,
            self.tip.shape,
            self.tip.upper_diameter,
            self.tip.lower_diameter,
            self.tip.length,
            self.tip.contact_shape,
            self.tip.contact_diameter,
            self.tip.contact_depth,
            self.branch_angle_deg,
            self.branch_diameter,
            self.trunk_diameter,
            self.trunk_diameter_max,
            self.foot_diameter,
            self.base.enabled,
            self.base.plate_thickness,
            self.base.margin,
            self.base.chamfer_deg,
            self.base.raft,
            self.enabled
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_match_the_reference_panel() {
        let c = SupportConfig::default();
        assert_eq!(c.height_from_platform, 6.0);
        assert_eq!(c.touch_tip_distance, Density::High.spacing());
        assert_eq!(c.overhang_angle_deg, 45.0);
        assert!(c.overhangs_only, "the reference panel has it checked");
        assert_eq!(c.tip.upper_diameter, 0.50);
        assert_eq!(c.tip.lower_diameter, 0.75);
        assert_eq!(c.tip.length, 2.5);
        assert_eq!(c.tip.contact_diameter, 0.8);
        assert_eq!(c.tip.contact_depth, 0.4);
        assert_eq!(c.branch_angle_deg, 45.0);
        assert_eq!(c.branch_diameter, 0.85);
        assert_eq!(c.trunk_diameter, 0.85);
        assert_eq!(c.foot_diameter, 1.5);
        assert_eq!(c.base.plate_thickness, 0.85);
        assert_eq!(c.base.margin, 2.5);
        assert_eq!(c.base.chamfer_deg, 15.0);
        assert!(!c.base.raft);
    }

    #[test]
    fn the_density_row_widens_from_high_to_low() {
        assert!(Density::Low.spacing() > Density::Medium.spacing());
        assert!(Density::Medium.spacing() > Density::High.spacing());
        assert_eq!(Density::High.spacing(), 4.0, "the reference spacing");
        assert_eq!(Density::ALL.len(), 3);
    }

    #[test]
    fn a_preset_sizes_struts_and_leaves_the_density_alone() {
        // Which side owns which number: the preset sets thicknesses, the
        // density row sets the spacing, and neither rewrites the other.
        let mut c = SupportConfig::default();
        let spacing = c.touch_tip_distance;
        let height = c.height_from_platform;
        let overhang = c.overhang_angle_deg;
        c.apply_preset(Preset::Heavy);
        assert!(c.trunk_diameter > SupportConfig::default().trunk_diameter);
        assert_eq!(
            c.touch_tip_distance, spacing,
            "a preset must not move the density"
        );
        assert_eq!(c.preset, Preset::Heavy);

        c.apply_preset(Preset::Light);
        assert!((c.trunk_diameter - 0.7).abs() < 1e-4);
        assert!(
            (c.foot_diameter - 1.8).abs() < 1e-4,
            "Light must not keep Heavy's numbers"
        );
        assert_eq!(c.touch_tip_distance, spacing);
        assert_eq!(c.height_from_platform, height);
        assert_eq!(c.overhang_angle_deg, overhang);
        assert!(c.enabled, "the on/off switch is not a preset's business");
    }

    #[test]
    fn derived_numbers_follow_the_tip_distance() {
        let c = SupportConfig::default();
        assert!((c.min_tip_spacing() - c.touch_tip_distance).abs() < 1e-4);
        assert!(c.max_unsupported_radius() > c.touch_tip_distance);
        assert!((c.cluster_radius() - c.touch_tip_distance * 2.5).abs() < 1e-4);
    }

    #[test]
    fn a_busier_trunk_grows_towards_its_maximum() {
        let c = SupportConfig::default();
        assert!((c.trunk_diameter_for(1) - 0.85).abs() < 1e-4);
        assert!((c.trunk_diameter_for(100) - 1.275).abs() < 1e-4);
        assert!(c.trunk_diameter_for(4) > c.trunk_diameter_for(2));
    }

    #[test]
    fn settings_are_written_out_and_are_serde_ready() {
        let c = SupportConfig::default();
        let text = c.to_json();
        assert!(text.contains("\"touch_tip_distance\": 4"));
        assert!(text.contains("\"plate_thickness\": 0.85"));
        assert!(text.contains("\"raft\": false"));
        // The serde derives exist, so any serde format can save and load it.
        fn serde_ready<T: serde::Serialize + serde::de::DeserializeOwned>() {}
        serde_ready::<SupportConfig>();
    }

    #[test]
    fn an_old_project_without_the_raft_key_loads_without_raft() {
        // The raft switch is gone from the panel; older projects must still
        // load, and the default is now the split-pad base.
        let base: BaseConfig = serde_json::from_str(
            "{\"enabled\": true, \"plate_thickness\": 0.8, \"margin\": 1.0, \"chamfer_deg\": 45.0}",
        )
        .expect("a pre-raft base loads");
        assert!(!base.raft, "a missing raft key means split pads");
    }
}
