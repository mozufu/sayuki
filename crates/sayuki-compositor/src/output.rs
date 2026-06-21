use smithay::{
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::DisplayHandle,
    utils::{Physical, Size, Transform},
};

use crate::state::SayukiState;

pub(crate) const OUTPUT_REFRESH_MHZ: i32 = 60_000;

/// Static, config-driven per-output policy (milestone 5b). Resolved by output
/// name; the default is native scale 1 / `Transform::Normal`. No dynamic
/// re-scale — fractional/dynamic scale is deferred to milestone 6.
#[derive(Clone, Debug)]
pub(crate) struct OutputPolicy {
    pub(crate) name: String,
    pub(crate) scale: i32,
    pub(crate) transform: Transform,
}

impl OutputPolicy {
    pub(crate) fn new(
        name: String,
        scale: Option<i32>,
        transform: Option<&str>,
    ) -> Result<Self, String> {
        let transform = match transform {
            None => Transform::Normal,
            Some(value) => parse_transform(value)
                .ok_or_else(|| format!("output `{name}` has unknown transform `{value}`"))?,
        };
        Ok(Self {
            name,
            scale: scale.unwrap_or(1).max(1),
            transform,
        })
    }
}

/// The `(scale, transform)` to apply to `output_name`, defaulting to native.
pub(crate) fn resolve_policy(policies: &[OutputPolicy], output_name: &str) -> (i32, Transform) {
    policies
        .iter()
        .find(|policy| policy.name == output_name)
        .map(|policy| (policy.scale, policy.transform))
        .unwrap_or((1, Transform::Normal))
}

/// Apply the resolved scale/transform to `output`, preserving its mode and
/// location. Idempotent, so it can be re-run after hotplug/session changes.
pub(crate) fn apply_policy(output: &Output, policies: &[OutputPolicy]) {
    let (scale, transform) = resolve_policy(policies, &output.name());
    output.change_current_state(None, Some(transform), Some(Scale::Integer(scale)), None);
}

fn parse_transform(value: &str) -> Option<Transform> {
    let transform = match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "normal" | "0" => Transform::Normal,
        "90" => Transform::_90,
        "180" => Transform::_180,
        "270" => Transform::_270,
        "flipped" | "flipped-0" => Transform::Flipped,
        "flipped-90" => Transform::Flipped90,
        "flipped-180" => Transform::Flipped180,
        "flipped-270" => Transform::Flipped270,
        _ => return None,
    };
    Some(transform)
}

pub(crate) fn create_nested_output(
    display_handle: &DisplayHandle,
    size: Size<i32, Physical>,
) -> Output {
    let output = Output::new(
        "sayuki-nested-0".into(),
        PhysicalProperties {
            size: (340, 210).into(),
            subpixel: Subpixel::Unknown,
            make: "Sayuki".into(),
            model: "Nested winit".into(),
        },
    );

    output.create_global::<SayukiState>(display_handle);
    configure_output(&output, size);

    output
}

pub(crate) fn configure_output(output: &Output, size: Size<i32, Physical>) {
    let mode = Mode {
        size,
        refresh: OUTPUT_REFRESH_MHZ,
    };

    output.set_preferred(mode);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_transform_accepts_known_values() {
        assert_eq!(parse_transform("normal"), Some(Transform::Normal));
        assert_eq!(parse_transform("90"), Some(Transform::_90));
        assert_eq!(parse_transform("flipped_180"), Some(Transform::Flipped180));
        assert_eq!(parse_transform("bogus"), None);
    }

    #[test]
    fn output_policy_defaults_and_clamps_scale() {
        let policy = OutputPolicy::new("eDP-1".to_owned(), None, None).expect("policy");
        assert_eq!(policy.scale, 1);
        assert_eq!(policy.transform, Transform::Normal);

        let clamped = OutputPolicy::new("eDP-1".to_owned(), Some(0), Some("90")).expect("policy");
        assert_eq!(clamped.scale, 1);
        assert_eq!(clamped.transform, Transform::_90);

        assert!(OutputPolicy::new("eDP-1".to_owned(), Some(2), Some("nope")).is_err());
    }

    #[test]
    fn resolve_policy_matches_by_name_else_native() {
        let policies = [OutputPolicy {
            name: "eDP-1".to_owned(),
            scale: 2,
            transform: Transform::_90,
        }];
        assert_eq!(resolve_policy(&policies, "eDP-1"), (2, Transform::_90));
        assert_eq!(
            resolve_policy(&policies, "HDMI-A-1"),
            (1, Transform::Normal)
        );
    }
}
