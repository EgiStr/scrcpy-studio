//! Live monitoring of phones on a USB cable: health, the app in front, and a small
//! screenshot, each read with an explicit serial.

use crate::error::Result;
use base64::Engine;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsbDevice {
    pub serial: String,
    /// adb state: device, unauthorized, offline.
    pub state: String,
    pub model: Option<String>,
    pub android: Option<String>,
    pub battery: Option<u32>,
    pub temperature_c: Option<f32>,
    /// `package/activity` in front.
    pub foreground: Option<String>,
}

/// A phone on a cable: not an emulator and not a Wi-Fi connection (`ip:port` or mDNS).
pub fn is_cable(serial: &str) -> bool {
    !serial.starts_with("emulator-") && !serial.contains(':') && !serial.contains("._adb")
}

const STATUS_SCRIPT: &str = "echo M=$(getprop ro.product.model); echo A=$(getprop ro.build.version.release); \
dumpsys battery | grep -E '^ *(level|temperature):'; \
dumpsys activity activities | grep -m1 -E 'topResumedActivity|mResumedActivity'";

#[derive(Debug, Default, PartialEq)]
struct Status {
    model: Option<String>,
    android: Option<String>,
    battery: Option<u32>,
    temperature_c: Option<f32>,
    foreground: Option<String>,
}

fn parse_status(text: &str) -> Status {
    let mut st = Status::default();
    let some = |v: &str| Some(v.trim().to_string()).filter(|v| !v.is_empty());
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("M=") {
            st.model = some(v);
        } else if let Some(v) = line.strip_prefix("A=") {
            st.android = some(v);
        } else if let Some(v) = line.strip_prefix("level:") {
            st.battery = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("temperature:") {
            st.temperature_c = v.trim().parse::<f32>().ok().map(|t| t / 10.0);
        } else if line.contains("ResumedActivity") && st.foreground.is_none() {
            st.foreground = line.split_whitespace().find(|w| w.contains('/')).map(|w| w.trim_end_matches('}').to_string());
        }
    }
    st
}

/// Every phone on a cable, with its health when adb can reach it.
pub fn usb_devices(adb: &Path) -> Result<Vec<UsbDevice>> {
    Ok(crate::adb::devices(adb)?
        .into_iter()
        .filter(|d| is_cable(&d.serial))
        .map(|d| {
            let st = if d.state == "device" {
                crate::adb::shell(adb, &d.serial, STATUS_SCRIPT).map(|t| parse_status(&t)).unwrap_or_default()
            } else {
                Status::default()
            };
            UsbDevice {
                model: st.model.or(d.model),
                serial: d.serial,
                state: d.state,
                android: st.android,
                battery: st.battery,
                temperature_c: st.temperature_c,
                foreground: st.foreground,
            }
        })
        .collect())
}

/// The screen as a `data:` URL, scaled to `width` px so polling it stays cheap.
pub fn screen_data_url(adb: &Path, serial: &str, width: u32) -> Result<String> {
    let png = crate::adb::screencap(adb, serial)?;
    let (mime, bytes) = shrink(&png, width).unwrap_or(("image/png", png));
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(windows)]
fn shrink(png: &[u8], width: u32) -> Option<(&'static str, Vec<u8>)> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let small = img.resize(width, width * 4, image::imageops::FilterType::Triangle).to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70).encode_image(&small).ok()?;
    Some(("image/jpeg", out.into_inner()))
}

#[cfg(not(windows))]
fn shrink(_png: &[u8], _width: u32) -> Option<(&'static str, Vec<u8>)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cable_phones_are_monitored() {
        assert!(is_cable("33061JEHN09954"));
        assert!(!is_cable("emulator-5554"));
        assert!(!is_cable("192.168.1.7:5555"));
        assert!(!is_cable("adb-33061JEHN09954-abc._adb-tls-connect._tcp"));
    }

    #[test]
    fn status_reads_model_battery_and_front_app() {
        let text = "M=Pixel 7a\nA=17\n  level: 84\n  temperature: 312\n  topResumedActivity=ActivityRecord{1a2b u0 com.telkomsel.telkomselcm/com.telkomsel.mytelkomsel.view.home.HomeActivity t42}\n";
        let st = parse_status(text);
        assert_eq!(st.model.as_deref(), Some("Pixel 7a"));
        assert_eq!(st.android.as_deref(), Some("17"));
        assert_eq!(st.battery, Some(84));
        assert_eq!(st.temperature_c, Some(31.2));
        assert_eq!(st.foreground.as_deref(), Some("com.telkomsel.telkomselcm/com.telkomsel.mytelkomsel.view.home.HomeActivity"));
        assert_eq!(parse_status("M=\nA=\n"), Status::default());
    }

    #[test]
    #[cfg(windows)]
    fn screenshots_are_scaled_down_to_jpeg() {
        let img = image::RgbImage::from_pixel(1080, 2400, image::Rgb([200, 30, 40]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let (mime, jpg) = shrink(png.get_ref(), 320).unwrap();
        assert_eq!(mime, "image/jpeg");
        let back = image::load_from_memory(&jpg).unwrap();
        assert_eq!((back.width(), back.height()), (320, 711));
    }

    /// `cargo test --lib usb_monitor -- --ignored --nocapture` with a phone on a cable.
    #[test]
    #[ignore = "needs a phone on a USB cable"]
    fn usb_monitor_reads_a_real_phone() {
        let adb = crate::emulator::locate_sdk().and_then(|s| s.adb).unwrap_or_else(|| "adb".into());
        let phones = usb_devices(&adb).unwrap();
        println!("{phones:#?}");
        let phone = phones.iter().find(|d| d.state == "device").expect("a phone on a cable");
        assert!(phone.model.is_some() && phone.android.is_some() && phone.battery.is_some());
        let url = screen_data_url(&adb, &phone.serial, 360).unwrap();
        println!("screen: {} KB", url.len() / 1024);
        assert!(url.starts_with("data:image/jpeg;base64,"));
    }
}
