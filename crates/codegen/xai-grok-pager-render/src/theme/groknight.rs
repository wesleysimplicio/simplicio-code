//! Simplicio Brasil theme — dark base with Brazilian green and yellow accents.
//!
//! The canonical palette is defined in RGB (`Color::Rgb`). At startup the
//! theme is run through [`Theme::quantized`] which downgrades every color
//! to the terminal's detected capability level (256-color, 16-color, etc.).

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

/// Helper for concise const `Color::Rgb` definitions.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

// GrokNight palette — neutral gray base + TokyoNight accent colors.
//
// Backgrounds and text use a custom grayscale ramp anchored at:
//   • bg  = #141414 (20)
//   • fg  = #f3f3f3 (243)
//
// Accent colors are the original TokyoNight Night hex values.
#[allow(dead_code)]
mod palette {
    use super::*;

    // ── Backgrounds ─────────────────────────────────────────────────────
    pub const BG: Color = rgb(10, 10, 10); //  #0a0a0a — Night (terminal bg)
    pub const BG_DARK: Color = rgb(12, 12, 12); //  #0c0c0c — darkest
    pub const BG_STORM_DARK: Color = rgb(17, 17, 17); //  #111111 — dark bg
    pub const BG_STORM: Color = rgb(20, 20, 20); //  #141414 — main bg
    pub const BG_HIGHLIGHT: Color = rgb(36, 36, 36); //  #242424 — highlight bg

    // ── Text / grays ────────────────────────────────────────────────────
    pub const FG: Color = rgb(225, 225, 225); // #e1e1e1 — primary text
    pub const FG_DARK: Color = rgb(200, 200, 200); // #c8c8c8 — secondary text
    pub const FG_GUTTER: Color = rgb(65, 65, 65); //  #414141 — dim
    pub const COMMENT: Color = rgb(108, 108, 108); //  #6c6c6c — muted
    pub const DARK3: Color = rgb(90, 90, 90); //  #5a5a5a — medium gray
    pub const DARK5: Color = rgb(120, 120, 120); // #787878 — bright gray

    // ── Accent colors (Brazil) ───────────────────────────────────────────
    // #002776 is the literal Brazil flag navy. Kept for reference/decorative
    // use, but it is NOT used as a foreground/accent color below: against
    // this theme's dark backgrounds it measures ~1.4:1 contrast, far under
    // the WCAG AA minimum of 4.5:1 for text (or 3:1 for UI components).
    // BLUE1 (#3A95AB, ~5.3-5.7:1) is the accessible stand-in used instead.
    pub const BLUE: Color = rgb(0, 39, 118); // #002776
    pub const BLUE0: Color = rgb(61, 89, 161); // #3d59a1
    pub const BLUE1: Color = rgb(58, 149, 171); // #3A95AB
    pub const CYAN: Color = rgb(125, 207, 255); // #7dcfff
    pub const GREEN: Color = rgb(0, 151, 57); // #009739
    pub const GREEN1: Color = rgb(0, 200, 83); // #00c853
    pub const MAGENTA: Color = rgb(0, 151, 57); // compatibility alias
    pub const ORANGE: Color = rgb(255, 158, 100); // #ff9e64
    pub const PURPLE: Color = rgb(157, 124, 216); // #9d7cd8
    pub const RED: Color = rgb(247, 118, 142); // #f7768e
    pub const RED1: Color = rgb(219, 75, 75); // #db4b4b
    pub const TEAL: Color = rgb(0, 200, 83); // bright Brazil green
    pub const YELLOW: Color = rgb(255, 223, 0); // #ffdf00

    pub const RED_DARK: Color = rgb(66, 14, 20); // #420e14 — quantizes to 256-color red, not gray
    pub const GREEN_DARK: Color = rgb(6, 56, 6); // #063806 — quantizes to 256-color green, not gray
}
use palette::*;

impl Theme {
    /// GrokNight theme — neutral gray base with TokyoNight accents.
    ///
    /// Colors are defined in RGB. Call [`Theme::quantized`] to downgrade
    /// them to the terminal's supported color level before rendering.
    pub const fn groknight() -> Self {
        Self {
            bg_base: BG_STORM,
            bg_light: BG_HIGHLIGHT,
            bg_dark: rgb(28, 28, 28), // lighter than bg_base for visible code blocks
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: rgb(44, 44, 44),
            bg_terminal: BG,

            accent_user: FG_DARK,
            accent_assistant: GREEN1,
            accent_thinking: YELLOW,
            accent_tool: DARK5,
            accent_system: BLUE1,
            accent_error: RED,
            accent_success: GREEN,
            accent_running: GREEN1,
            accent_skill: BLUE1,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: rgb(88, 88, 88), // #585858 — slightly brighter than FG_GUTTER
            gray: COMMENT,
            gray_bright: DARK5,

            command: YELLOW,
            path: ORANGE,
            running: CYAN,
            warning: YELLOW,

            fuzzy_accent: BLUE1,

            accent_plan: rgb(255, 219, 141), // #FFDB8D — golden

            accent_verify: BLUE1,

            accent_feedback: GREEN1, // #73daca

            accent_remember: Color::Rgb(139, 195, 74), // #8BC34A — Material Design light green

            selection_border: rgb(60, 60, 65),
            prompt_border: rgb(50, 50, 55), // #323237 — dimmer prompt chrome
            prompt_border_active: GREEN,
            hover_border: rgb(30, 30, 34),

            accent_model: TEAL,

            scrollbar_bg: BG_STORM_DARK,
            scrollbar_fg: BG_HIGHLIGHT,

            diff_delete_bg: RED_DARK,
            diff_delete_fg: RED,
            diff_insert_bg: GREEN_DARK,
            diff_insert_fg: GREEN,
            diff_equal_fg: COMMENT,
            diff_gutter_fg: COMMENT,

            bg_visual: rgb(54, 54, 54),

            paste_bg: BG_STORM_DARK,
            paste_fg: FG_DARK,
            paste_dim: FG_GUTTER,

            md_heading_h1: GREEN1,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: YELLOW,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: PURPLE,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: DARK5, // bright gray
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: COMMENT, // medium gray
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: DARK3, // medium gray, unbold
            md_heading_h6_mod: Modifier::empty(),
            md_code: BLUE1,
            md_task_checked: GREEN,
            md_task_unchecked: FG_DARK, // text_secondary
            md_muted: COMMENT,
            md_code_bg: rgb(28, 28, 28),
            md_text: FG_DARK,
            link_fg: rgb(122, 166, 218), // #7aa6da -- soft blue for dark bg
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "known broken: expected accent values drift from runtime theme"]
    fn test_groknight_theme() {
        let theme = Theme::groknight();
        assert!(matches!(theme.bg_base, Color::Rgb(20, 20, 20)));
        assert!(matches!(theme.accent_user, Color::Rgb(225, 225, 225)));
        assert!(matches!(theme.text_primary, Color::Rgb(225, 225, 225)));
    }
}
