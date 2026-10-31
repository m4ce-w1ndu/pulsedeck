use crate::json::{escape_json, extract_json_str, extract_json_u64};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ThemeColors {
    pub preset_name: String,
    pub bg_main: [u8; 3],
    pub bg_panel: [u8; 3],
    pub bg_card: [u8; 3],
    pub border: [u8; 3],
    pub accent_primary: [u8; 3],
    pub accent_secondary: [u8; 3],
    pub accent_warn: [u8; 3],
    pub accent_danger: [u8; 3],
    pub text_main: [u8; 3],
    pub text_dim: [u8; 3],
}

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub theme: ThemeColors,
    pub font_family: String,
    pub base_font_size: u32,
    pub refresh_ms: u64,
    pub compact_rows: bool,
    pub show_graph_grid: bool,
    pub hide_kernel_threads: bool,
}

pub const THEME_PRESETS: [&str; 8] = [
    "Fluent Modern",
    "KDE Breeze",
    "VFD Phosphor Green",
    "Amber CRT",
    "Cyberpunk Cyan",
    "Nordic Frost",
    "Solarized Daylight",
    "Crimson Ops",
];

pub const FONT_PRESETS: [&str; 9] = [
    "Sans-Serif",
    "Noto Sans",
    "Segoe UI",
    "DejaVu Sans Mono",
    "JetBrains Mono",
    "Fira Code",
    "Ubuntu Mono",
    "Liberation Mono",
    "Consolas",
];

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeColors::from_preset("VFD Phosphor Green"),
            font_family: String::from("DejaVu Sans Mono"),
            base_font_size: 13,
            refresh_ms: 1000,
            compact_rows: false,
            show_graph_grid: true,
            hide_kernel_threads: true,
        }
    }
}

impl ThemeColors {
    pub fn from_preset(name: &str) -> Self {
        match name {
            "Fluent Modern" | "Fluent" | "fluent" => Self {
                preset_name: String::from("Fluent Modern"),
                bg_main: [32, 32, 32],
                bg_panel: [40, 40, 40],
                bg_card: [45, 45, 45],
                border: [60, 60, 60],
                accent_primary: [96, 205, 255],
                accent_secondary: [112, 165, 253],
                accent_warn: [252, 225, 0],
                accent_danger: [255, 153, 164],
                text_main: [255, 255, 255],
                text_dim: [160, 160, 160],
            },
            "KDE Breeze" | "KDE" | "kde" => Self {
                preset_name: String::from("KDE Breeze"),
                bg_main: [35, 38, 41],
                bg_panel: [27, 30, 32],
                bg_card: [42, 46, 50],
                border: [56, 61, 66],
                accent_primary: [61, 174, 233],
                accent_secondary: [29, 211, 192],
                accent_warn: [253, 188, 75],
                accent_danger: [237, 21, 21],
                text_main: [252, 252, 252],
                text_dim: [158, 166, 173],
            },
            "Amber CRT" => Self {
                preset_name: String::from("Amber CRT"),
                bg_main: [13, 9, 5],
                bg_panel: [22, 15, 8],
                bg_card: [30, 20, 10],
                border: [68, 45, 18],
                accent_primary: [255, 176, 0],
                accent_secondary: [255, 213, 79],
                accent_warn: [255, 140, 0],
                accent_danger: [255, 68, 68],
                text_main: [255, 230, 179],
                text_dim: [176, 138, 84],
            },
            "Cyberpunk Cyan" => Self {
                preset_name: String::from("Cyberpunk Cyan"),
                bg_main: [7, 9, 19],
                bg_panel: [12, 16, 33],
                bg_card: [18, 24, 48],
                border: [38, 54, 102],
                accent_primary: [0, 240, 255],
                accent_secondary: [255, 42, 133],
                accent_warn: [255, 204, 0],
                accent_danger: [255, 51, 102],
                text_main: [224, 242, 254],
                text_dim: [125, 155, 196],
            },
            "Nordic Frost" => Self {
                preset_name: String::from("Nordic Frost"),
                bg_main: [36, 41, 51],
                bg_panel: [46, 52, 64],
                bg_card: [59, 66, 82],
                border: [76, 86, 106],
                accent_primary: [136, 192, 208],
                accent_secondary: [129, 161, 193],
                accent_warn: [235, 203, 139],
                accent_danger: [191, 97, 106],
                text_main: [236, 239, 244],
                text_dim: [163, 174, 194],
            },
            "Solarized Daylight" => Self {
                preset_name: String::from("Solarized Daylight"),
                bg_main: [244, 246, 249],
                bg_panel: [232, 236, 242],
                bg_card: [255, 255, 255],
                border: [198, 208, 220],
                accent_primary: [0, 122, 90],
                accent_secondary: [0, 102, 204],
                accent_warn: [196, 120, 0],
                accent_danger: [204, 34, 51],
                text_main: [24, 34, 46],
                text_dim: [96, 112, 130],
            },
            "Crimson Ops" => Self {
                preset_name: String::from("Crimson Ops"),
                bg_main: [11, 7, 8],
                bg_panel: [19, 11, 13],
                bg_card: [28, 15, 18],
                border: [66, 26, 33],
                accent_primary: [255, 51, 68],
                accent_secondary: [255, 136, 0],
                accent_warn: [255, 187, 0],
                accent_danger: [255, 34, 34],
                text_main: [242, 220, 224],
                text_dim: [168, 122, 130],
            },
            _ => Self {
                preset_name: String::from("VFD Phosphor Green"),
                bg_main: [8, 12, 10],
                bg_panel: [13, 21, 18],
                bg_card: [17, 28, 24],
                border: [31, 56, 46],
                accent_primary: [0, 255, 157],
                accent_secondary: [0, 229, 255],
                accent_warn: [255, 179, 0],
                accent_danger: [255, 68, 85],
                text_main: [216, 236, 228],
                text_dim: [122, 160, 144],
            },
        }
    }
}

pub fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let clean = s.trim().trim_start_matches('#');
    if clean.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
    let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
    let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
    Some([r, g, b])
}

impl AppSettings {
    pub fn config_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                return PathBuf::from(appdata).join("pulsedeck").join("settings.json");
            }
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("pulsedeck").join("settings.json");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("pulsedeck")
                .join("settings.json");
        }
        PathBuf::from("pulsedeck-settings.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        let Ok(content) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut s = Self::default();
        if let Some(preset) = extract_json_str(&content, "preset_name") {
            s.theme = ThemeColors::from_preset(&preset);
        }
        if let Some(hex) = extract_json_str(&content, "accent_primary") {
            if let Some(rgb) = parse_hex_color(&hex) {
                s.theme.accent_primary = rgb;
            }
        }
        if let Some(hex) = extract_json_str(&content, "accent_secondary") {
            if let Some(rgb) = parse_hex_color(&hex) {
                s.theme.accent_secondary = rgb;
            }
        }
        if let Some(hex) = extract_json_str(&content, "bg_main") {
            if let Some(rgb) = parse_hex_color(&hex) {
                s.theme.bg_main = rgb;
            }
        }
        if let Some(hex) = extract_json_str(&content, "bg_panel") {
            if let Some(rgb) = parse_hex_color(&hex) {
                s.theme.bg_panel = rgb;
            }
        }
        if let Some(hex) = extract_json_str(&content, "bg_card") {
            if let Some(rgb) = parse_hex_color(&hex) {
                s.theme.bg_card = rgb;
            }
        }
        if let Some(hex) = extract_json_str(&content, "text_main") {
            if let Some(rgb) = parse_hex_color(&hex) {
                s.theme.text_main = rgb;
            }
        }
        if let Some(font) = extract_json_str(&content, "font_family") {
            if !font.is_empty() {
                s.font_family = font;
            }
        }
        if let Some(sz) = extract_json_u64(&content, "base_font_size") {
            s.base_font_size = (sz as u32).clamp(9, 24);
        }
        if let Some(ms) = extract_json_u64(&content, "refresh_ms") {
            s.refresh_ms = ms.clamp(200, 10_000);
        }
        s.compact_rows = content.contains("\"compact_rows\":true");
        if content.contains("\"show_graph_grid\":false") {
            s.show_graph_grid = false;
        }
        if content.contains("\"hide_kernel_threads\":false") {
            s.hide_kernel_threads = false;
        }
        s
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = format!(
            "{{\n  \"preset_name\":\"{}\",\n  \"accent_primary\":\"{}\",\n  \"accent_secondary\":\"{}\",\n  \"bg_main\":\"{}\",\n  \"bg_panel\":\"{}\",\n  \"bg_card\":\"{}\",\n  \"text_main\":\"{}\",\n  \"font_family\":\"{}\",\n  \"base_font_size\":{},\n  \"refresh_ms\":{},\n  \"compact_rows\":{},\n  \"show_graph_grid\":{},\n  \"hide_kernel_threads\":{}\n}}\n",
            escape_json(&self.theme.preset_name),
            rgb_to_hex(self.theme.accent_primary),
            rgb_to_hex(self.theme.accent_secondary),
            rgb_to_hex(self.theme.bg_main),
            rgb_to_hex(self.theme.bg_panel),
            rgb_to_hex(self.theme.bg_card),
            rgb_to_hex(self.theme.text_main),
            escape_json(&self.font_family),
            self.base_font_size,
            self.refresh_ms,
            self.compact_rows,
            self.show_graph_grid,
            self.hide_kernel_threads,
        );
        fs::write(&path, json)?;
        Ok(path)
    }
}
