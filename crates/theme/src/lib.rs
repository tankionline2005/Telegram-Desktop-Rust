//! Colors of the original client, generated at build time from tdesktop's palette
//! (`assets/theme/colors.palette`, vendored from `lib_ui`) and its built-in night theme.
//! Both files are updated by the upstream tracker, so a color change in tdesktop reaches
//! this client without manual work.

/// A color as `0xRRGGBBAA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub fn rgba(self) -> u32 {
        self.0
    }

    pub fn alpha(self) -> u8 {
        (self.0 & 0xff) as u8
    }
}

include!(concat!(env!("OUT_DIR"), "/palette.rs"));

/// tdesktop's default wallpaper (`Data::WallPaper::ConstructDefault`): a four-color
/// gradient with the `background.tgv` pattern at intensity 50.
pub const DEFAULT_WALLPAPER_COLORS: [Color; 4] = [
    Color(0xdbddbbff),
    Color(0x6ba587ff),
    Color(0xd5d88dff),
    Color(0x88b884ff),
];

/// The palette for a dark or light appearance.
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &NIGHT } else { &DAY }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_day_colors() {
        // Values from colors.palette.
        assert_eq!(DAY.window_bg, Color(0xffffffff));
        assert_eq!(DAY.window_bg_active, Color(0x40a7e3ff));
        assert_eq!(DAY.msg_out_bg, Color(0xeffddeff));
        // Aliases resolve: dialogsBg = windowBg.
        assert_eq!(DAY.dialogs_bg, DAY.window_bg);
        assert_eq!(DAY.shadow_fg, Color(0x00000018));
        const { assert!(COLOR_COUNT > 500) };
        // night.tdesktop-theme's background.png is a #0e1621 tile.
        assert_eq!(NIGHT_CHAT_BACKGROUND, Color(0x0e1621ff));
    }

    #[test]
    fn night_overrides_and_derives() {
        assert_eq!(NIGHT.window_bg, Color(0x17212bff));
        assert_ne!(NIGHT.msg_out_bg, DAY.msg_out_bg);
        // Keys the night theme does not set derive from its own values.
        assert_eq!(NIGHT.dialogs_bg, NIGHT.window_bg);
    }
}
