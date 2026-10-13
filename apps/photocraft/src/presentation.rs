//! Launch overrides for how frames reach the display, so testers can compare input-to-pixel
//! latency without a rebuild (#1343): `PHOTOCRAFT_PRESENT_MODE` (`fifo`, `mailbox`, `immediate`,
//! `autovsync`, `autonovsync`) and `PHOTOCRAFT_FRAME_LATENCY` (`1` to `3`, frames the swap chain may
//! queue). Unset, the platform default applies. A value that isn't one of these is ignored with a
//! warning in the log, never a failed start. The result is shown in the stroke trace and in
//! Help › System Info.

use eframe::egui_wgpu::SurfaceConfig;
use eframe::wgpu::PresentMode;

/// The present mode and frame latency this launch asked for (`None`: the default).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Overrides {
    pub mode: Option<PresentMode>,
    pub latency: Option<u32>,
}

/// Parse the two variables' values. Returns the overrides and a warning per rejected value.
pub fn parse(mode: Option<&str>, latency: Option<&str>) -> (Overrides, Vec<String>) {
    let mut warnings = Vec::new();
    let mode = mode.map(str::trim).filter(|v| !v.is_empty()).and_then(|v| {
        let m = match v.to_ascii_lowercase().as_str() {
            "fifo" | "vsync" => PresentMode::Fifo,
            "mailbox" => PresentMode::Mailbox,
            "immediate" => PresentMode::Immediate,
            "autovsync" => PresentMode::AutoVsync,
            "autonovsync" => PresentMode::AutoNoVsync,
            _ => {
                warnings.push(format!("PHOTOCRAFT_PRESENT_MODE={v:?} isn't fifo, mailbox, immediate, autovsync or autonovsync; ignored"));
                return None;
            }
        };
        Some(m)
    });
    let latency = latency.map(str::trim).filter(|v| !v.is_empty()).and_then(|v| match v.parse::<u32>() {
        Ok(n @ 1..=3) => Some(n),
        _ => {
            warnings.push(format!("PHOTOCRAFT_FRAME_LATENCY={v:?} isn't 1, 2 or 3; ignored"));
            None
        }
    });
    (Overrides { mode, latency }, warnings)
}

impl Overrides {
    /// Read `PHOTOCRAFT_PRESENT_MODE` and `PHOTOCRAFT_FRAME_LATENCY`, logging rejected values.
    pub fn from_env() -> Self {
        let (o, warnings) = parse(std::env::var("PHOTOCRAFT_PRESENT_MODE").ok().as_deref(), std::env::var("PHOTOCRAFT_FRAME_LATENCY").ok().as_deref());
        for w in warnings {
            log::warn!("{w}");
        }
        o
    }

    /// `surface` with the overrides applied.
    pub fn apply(&self, surface: SurfaceConfig) -> SurfaceConfig {
        SurfaceConfig {
            present_mode: self.mode.unwrap_or(surface.present_mode),
            desired_maximum_frame_latency: self.latency.or(surface.desired_maximum_frame_latency),
        }
    }
}

/// How `surface` presents, for the stroke trace and System Info: "fifo, latency 2", with
/// "(override)" after a value an environment variable set. wgpu's GL backend ignores the latency.
pub fn describe(surface: &SurfaceConfig, overrides: &Overrides) -> String {
    let mode = match surface.present_mode {
        PresentMode::AutoVsync => "autovsync",
        PresentMode::AutoNoVsync => "autonovsync",
        PresentMode::Fifo => "fifo",
        PresentMode::FifoRelaxed => "fiforelaxed",
        PresentMode::Immediate => "immediate",
        PresentMode::Mailbox => "mailbox",
    };
    let latency = surface.desired_maximum_frame_latency.map_or("default".to_string(), |n| n.to_string());
    let tag = |set: bool| if set { " (override)" } else { "" };
    format!("{mode}{}, latency {latency}{}", tag(overrides.mode.is_some()), tag(overrides.latency.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_parse_and_bad_ones_are_ignored_with_a_warning() {
        assert_eq!(parse(None, None), (Overrides::default(), vec![]));
        assert_eq!(parse(Some(""), Some(" ")), (Overrides::default(), vec![]));
        let (o, w) = parse(Some(" Mailbox "), Some("1"));
        assert_eq!((o.mode, o.latency, w.len()), (Some(PresentMode::Mailbox), Some(1), 0));
        assert_eq!(parse(Some("vsync"), None).0.mode, Some(PresentMode::Fifo));
        assert_eq!(parse(Some("IMMEDIATE"), None).0.mode, Some(PresentMode::Immediate));
        for (mode, latency) in
            [(Some("fast"), None), (None, Some("0")), (None, Some("4")), (None, Some("-1")), (None, Some("one")), (None, Some("99999999999"))]
        {
            let (o, w) = parse(mode, latency);
            assert_eq!((o, w.len()), (Overrides::default(), 1), "{mode:?} {latency:?}");
        }
    }

    #[test]
    fn overrides_replace_only_what_they_set_and_are_described() {
        let default = SurfaceConfig::HIGH_THROUGHPUT;
        assert_eq!(Overrides::default().apply(default), default);
        let latency_only = Overrides { mode: None, latency: Some(1) };
        let s = latency_only.apply(default);
        assert_eq!((s.present_mode, s.desired_maximum_frame_latency), (PresentMode::AutoVsync, Some(1)));
        assert_eq!(describe(&s, &latency_only), "autovsync, latency 1 (override)");
        let mailbox = Overrides { mode: Some(PresentMode::Mailbox), latency: None };
        assert_eq!(describe(&mailbox.apply(default), &mailbox), "mailbox (override), latency 2");
        let none = SurfaceConfig { present_mode: PresentMode::Fifo, desired_maximum_frame_latency: None };
        assert_eq!(describe(&none, &Overrides::default()), "fifo, latency default");
    }
}
