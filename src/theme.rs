//! Design tokens: a zero-hue neutral scale, sharp corners, native UI font.
//!
//! The source tokens are authored in OKLCH (shadcn's neutral base). GPUI has no
//! OKLCH constructor, so they are pre-converted to sRGB here; the OKLCH value is
//! kept alongside each entry so the two stay traceable.

use std::sync::LazyLock;

use gpui::{Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Hsla, Rgba, hsla, rgb};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Self::Light => Palette::light(),
            Self::Dark => Palette::dark(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Hsla,
    pub foreground: Hsla,
    pub card: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    /// Retained shadcn slot; active rows use `select_strong` so active never
    /// equals the `muted` hover fill in either mode.
    #[allow(dead_code)]
    pub accent: Hsla,
    pub border: Hsla,
    pub destructive: Hsla,
    pub sidebar: Hsla,
    pub sidebar_border: Hsla,
    pub chart_bar: Hsla,
    pub chart_bar_track: Hsla,
    /// Selected-row fill. Deliberately a step stronger than `accent`.
    pub select_strong: Hsla,
    pub overlay: Hsla,
}

fn c(hex: u32) -> Hsla {
    let rgba: Rgba = rgb(hex);
    rgba.into()
}

impl Palette {
    fn light() -> Self {
        Self {
            background: c(0xffffff),       // oklch(1 0 0)
            foreground: c(0x0a0a0a),       // oklch(0.145 0 0)
            card: c(0xffffff),             // oklch(1 0 0)
            muted: c(0xf5f5f5),            // oklch(0.97 0 0)
            muted_foreground: c(0x737373), // oklch(0.556 0 0)
            accent: c(0xf5f5f5),           // oklch(0.97 0 0)
            border: c(0xe5e5e5),           // oklch(0.922 0 0)
            destructive: c(0xe7000b),      // oklch(0.577 0.245 27.325)
            sidebar: c(0xfafafa),          // oklch(0.985 0 0)
            sidebar_border: c(0xe5e5e5),   // oklch(0.922 0 0)
            chart_bar: c(0x737373),        // oklch(0.556 0 0)
            chart_bar_track: c(0xe8e8e8),  // oklch(0.93 0 0)
            select_strong: c(0xe4e4e4),    // oklch(0.92 0 0)
            overlay: hsla(0., 0., 0., 0.4),
        }
    }

    fn dark() -> Self {
        Self {
            background: c(0x0a0a0a),               // oklch(0.145 0 0)
            foreground: c(0xfafafa),               // oklch(0.985 0 0)
            card: c(0x171717),                     // oklch(0.205 0 0)
            muted: c(0x262626),                    // oklch(0.269 0 0)
            muted_foreground: c(0xa1a1a1),         // oklch(0.708 0 0)
            accent: c(0x262626),                   // oklch(0.269 0 0)
            border: hsla(0., 0., 1., 0.1),         // oklch(1 0 0 / 10%)
            destructive: c(0xff6467),              // oklch(0.704 0.191 22.216)
            sidebar: c(0x171717),                  // oklch(0.205 0 0)
            sidebar_border: hsla(0., 0., 1., 0.1), // oklch(1 0 0 / 10%)
            chart_bar: c(0xa1a1a1),                // oklch(0.708 0 0)
            chart_bar_track: c(0x333333),          // oklch(0.32 0 0)
            select_strong: c(0x333333),            // oklch(0.32 0 0)
            overlay: hsla(0., 0., 0., 0.4),
        }
    }
}

/// The CSS `-apple-system, "Segoe UI Variable", Ubuntu, …` stack, resolved per
/// platform: GPUI takes one family plus an ordered fallback list.
static UI_FONT: LazyLock<Font> = LazyLock::new(|| {
    let (family, fallbacks): (&str, &[&str]) = if cfg!(windows) {
        ("Segoe UI Variable", &["Segoe UI", "Tahoma", "Arial"])
    } else if cfg!(target_os = "macos") {
        (".SystemUIFont", &["SF Pro Text", "Helvetica Neue", "Arial"])
    } else {
        ("Ubuntu", &["Roboto", "DejaVu Sans", "Helvetica", "Arial"])
    };
    Font {
        family: family.into(),
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(
            fallbacks.iter().map(|s| s.to_string()).collect(),
        )),
        weight: FontWeight::default(),
        style: FontStyle::default(),
    }
});

pub fn ui_font() -> Font {
    UI_FONT.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: Hsla, b: Hsla) -> bool {
        (a.h - b.h).abs() < 1e-6
            && (a.s - b.s).abs() < 1e-6
            && (a.l - b.l).abs() < 1e-6
            && (a.a - b.a).abs() < 1e-6
    }

    fn neutrals(p: &Palette) -> [(&'static str, Hsla); 11] {
        [
            ("background", p.background),
            ("foreground", p.foreground),
            ("card", p.card),
            ("muted", p.muted),
            ("muted_foreground", p.muted_foreground),
            ("accent", p.accent),
            ("sidebar", p.sidebar),
            ("chart_bar", p.chart_bar),
            ("chart_bar_track", p.chart_bar_track),
            ("select_strong", p.select_strong),
            ("overlay", p.overlay),
        ]
    }

    #[test]
    fn mode_toggle_is_an_involution() {
        assert_eq!(Mode::Light.toggled(), Mode::Dark);
        assert_eq!(Mode::Dark.toggled(), Mode::Light);
        assert_eq!(Mode::Light.toggled().toggled(), Mode::Light);
    }

    #[test]
    fn palette_follows_the_mode() {
        let light = Mode::Light.palette();
        let dark = Mode::Dark.palette();
        assert!(light.background.l > 0.9, "light background must be bright");
        assert!(
            dark.background.l < 0.1,
            "dark background must be near black"
        );
        assert!(light.foreground.l < dark.foreground.l, "text inverts");
    }

    #[test]
    fn neutral_scale_has_zero_saturation() {
        // Documented invariant: only `destructive` carries hue.
        for mode in [Mode::Light, Mode::Dark] {
            let p = mode.palette();
            for (name, token) in neutrals(&p) {
                assert!(
                    token.s.abs() < 1e-4,
                    "{mode:?} {name} must be zero-hue, saturated {}",
                    token.s
                );
            }
            assert!(
                p.destructive.s > 0.1,
                "{mode:?} destructive must carry the only hue"
            );
        }
    }

    #[test]
    fn select_strong_is_never_the_hover_fill() {
        // Active rows use `select_strong`, so it must stay distinct from the
        // `muted`/`accent` hover fill in both modes.
        for mode in [Mode::Light, Mode::Dark] {
            let p = mode.palette();
            assert!(!same(p.select_strong, p.muted), "{mode:?} select vs muted");
            assert!(
                !same(p.select_strong, p.accent),
                "{mode:?} select vs accent"
            );
            assert!(same(p.accent, p.muted), "{mode:?} accent mirrors muted");
        }
    }

    #[test]
    fn destructive_is_opaque_in_both_modes() {
        for mode in [Mode::Light, Mode::Dark] {
            let p = mode.palette();
            assert!(p.destructive.s > 0.1, "{mode:?} destructive carries hue");
            assert!((p.destructive.a - 1.0).abs() < 1e-4, "{mode:?} destructive");
        }
    }

    #[test]
    fn overlay_is_black_at_forty_percent() {
        for mode in [Mode::Light, Mode::Dark] {
            let o = mode.palette().overlay;
            assert!(o.l.abs() < 1e-4, "{mode:?} overlay must be black");
            assert!((o.a - 0.4).abs() < 1e-4, "{mode:?} overlay alpha");
        }
    }

    #[test]
    fn ui_font_matches_the_platform_and_has_fallbacks() {
        let f = ui_font();
        let expected = if cfg!(windows) {
            "Segoe UI Variable"
        } else if cfg!(target_os = "macos") {
            ".SystemUIFont"
        } else {
            "Ubuntu"
        };
        assert_eq!(f.family.to_string(), expected);
        let fallbacks = f.fallbacks.expect("a fallback list is always set");
        assert!(!fallbacks.0.is_empty(), "fallbacks must not be empty");
    }
}
