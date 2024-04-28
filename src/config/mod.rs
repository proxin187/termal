use crate::xlib;

use toml::Table;

use std::env;
use std::fs;

#[derive(Clone, Copy)]
pub struct UniColor {
    pub raw: xlib::Color,
    pub xft: x11::xft::XftColor,
}

impl PartialEq for UniColor {
    fn eq(&self, other: &UniColor) -> bool {
        self.raw == other.raw
    }

    fn ne(&self, other: &UniColor) -> bool {
        self.raw != other.raw
    }
}

pub struct Config {
    pub colors: Vec<UniColor>,
    pub tab_max: usize,
    pub scrollback: usize,
    pub font: String,
    pub bell: String,
    pub fg: UniColor,
    pub bg: UniColor,
}

impl Config {
    pub fn load(display: &xlib::Display) -> Result<Config, Box<dyn std::error::Error>> {
        let home = env::var("HOME")?;

        let colors = vec![
            "28-28-28", // black
            "cc-24-1d", // red
            "98-97-1a", // green
            "d6-5d-0e", // brown
            "45-85-88", // blue
            "b1-62-86", // magneta
            "83-a5-98", // cyan
            "eb-db-b2", // white
        ];

        if let Ok(content) = fs::read_to_string(format!("{}/.config/termal/config.toml", home)) {
            let config = content.parse::<Table>()?;
            let fg = xlib::Color::from_str(&Self::get_str(&config, "foreground", "d7-e0-da"))?;
            let bg = xlib::Color::from_str(&Self::get_str(&config, "background", "0d-16-17"))?;

            Ok(Config {
                colors: Self::load_colors(display, Self::get_colors(&config, colors)?.iter().map(|x| x.as_str()).collect::<Vec<&str>>())?,
                tab_max: Self::get_int(&config, "tab_max", 400),
                scrollback: Self::get_int(&config, "scrollback", 400),
                font: Self::get_str(&config, "font", "Iosevka Nerd Font Mono:style=Regular"),
                bell: Self::get_str(&config, "bell", "assets/pluh.wav"),
                fg: UniColor {
                    raw: fg,
                    xft: display.xft_color_alloc_value(fg)?,
                },
                bg: UniColor {
                    raw: bg,
                    xft: display.xft_color_alloc_value(bg)?,
                },
            })
        } else {
            Ok(Config {
                colors: Self::load_colors(display, colors)?,
                tab_max: 400,
                scrollback: 400,
                font: String::from("Iosevka Nerd Font Mono:style=Regular"),
                bell: String::from("assets/pluh.wav"),
                fg: UniColor {
                    raw: xlib::Color::from_str("d7-e0-da")?,
                    xft: display.xft_color_alloc_value(xlib::Color::from_str("d7-e0-da")?)?,
                },
                bg: UniColor {
                    raw: xlib::Color::from_str("0d-16-17")?,
                    xft: display.xft_color_alloc_value(xlib::Color::from_str("0d-16-17")?)?,
                },
            })
        }
    }

    fn load_colors(display: &xlib::Display, colors: Vec<&str>) -> Result<Vec<UniColor>, Box<dyn std::error::Error>> {
        let mut unicolors: Vec<UniColor> = Vec::new();

        for color in colors {
            let raw = xlib::Color::from_str(color)?;

            unicolors.push(UniColor { raw, xft: display.xft_color_alloc_value(raw)? });
        }

        Ok(unicolors)
    }

    fn get_colors(table: &toml::map::Map<String, toml::Value>, default: Vec<&str>) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        if let Some(colors) = table.get("colors") {
            Ok(colors.as_array().unwrap_or(&Vec::new()).iter().map(|x| x.as_str().unwrap_or_default().to_string()).collect::<Vec<String>>())
        } else {
            Ok(default.iter().map(|x| x.to_string()).collect::<Vec<String>>())
        }
    }

    fn get_str(table: &toml::map::Map<String, toml::Value>, key: &str, default: &str) -> String {
        table.get(key).map_or(default, |x| x.as_str().unwrap_or(default)).to_string()
    }

    fn get_int(config: &toml::map::Map<String, toml::Value>, key: &str, default: usize) -> usize {
        config.get(key).map_or(default, |x| x.as_integer().unwrap_or_default() as usize)
    }
}


