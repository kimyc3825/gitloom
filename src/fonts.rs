//! egui's bundled fonts have no Hangul/CJK glyphs, so Korean commit messages
//! would render as boxes. Add a system font as a fallback when one exists.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;

fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(windir) = std::env::var_os("WINDIR") {
        let fonts = PathBuf::from(windir).join("Fonts");
        paths.push(fonts.join("malgun.ttf")); // Malgun Gothic (Korean)
        paths.push(fonts.join("YuGothM.ttc")); // Japanese
        paths.push(fonts.join("msyh.ttc")); // Chinese
    }
    for p in [
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    ] {
        paths.push(PathBuf::from(p));
    }
    paths
}

pub fn install(ctx: &egui::Context) {
    let Some(bytes) = candidates().into_iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk-fallback".to_owned(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk-fallback".to_owned());
    }
    ctx.set_fonts(fonts);
}
