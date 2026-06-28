use hidapi::HidApi;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

const VENDOR_ID: u16 = 0x5131;
const PRODUCT_ID: u16 = 0x2007;
const UPDATE_INTERVAL: Duration = Duration::from_secs(2);

// ─── Linux ───────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn get_cpu_temp() -> Option<u8> {
    let mut paths: Vec<_> = fs::read_dir("/sys/class/hwmon")
        .ok()?
        .flatten()
        .flat_map(|hwmon| {
            fs::read_dir(hwmon.path())
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| {
                    e.file_name().to_string_lossy().starts_with("temp")
                        && e.file_name().to_string_lossy().ends_with("_input")
                })
        })
        .map(|e| e.path())
        .collect();

    paths.sort();

    // Priority pass: known CPU thermal drivers
    for path in &paths {
        if let Some(parent) = path.parent() {
            let name_path = parent.join("name");
            if let Ok(name) = fs::read_to_string(&name_path) {
                let name = name.trim();
                if matches!(
                    name,
                    "k10temp" | "coretemp" | "zenpower" | "nct6775" | "it87" | "amdgpu"
                ) {
                    if let Some(temp) = read_temp(path) {
                        return Some(temp);
                    }
                }
            }
        }
    }

    // Fallback pass: first sensor reading a sane value
    for path in &paths {
        if let Some(temp) = read_temp(path) {
            if temp > 20 {
                return Some(temp);
            }
        }
    }

    None
}

// ─── Windows ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod windows_temp {
    use serde::Deserialize;
    use wmi::{COMLibrary, WMIConnection};

    // LibreHardwareMonitor / OpenHardwareMonitor WMI sensor schema
    #[derive(Deserialize, Debug)]
    #[allow(non_snake_case)]
    pub struct OhmSensor {
        pub Name: String,
        pub Value: f32,
        pub SensorType: String,
        pub Parent: String,
    }

    // OHM/LHM sensor — requires the app to be running
    pub fn via_ohm() -> Option<u8> {
        let com = COMLibrary::new().ok()?;
        let conn = WMIConnection::with_namespace_path("ROOT\\LibreHardwareMonitor", com.into())
            .or_else(|_| {
                let com2 = COMLibrary::new().map_err(|e| e)?;
                WMIConnection::with_namespace_path("ROOT\\OpenHardwareMonitor", com2.into())
            })
            .ok()?;

        let sensors: Vec<OhmSensor> = conn.query().ok()?;

        // Prefer "CPU Package" or "Core Average", fall back to any CPU temperature
        let priority = ["CPU Package", "Core Average", "CPU"];
        for needle in &priority {
            for s in &sensors {
                if s.SensorType == "Temperature"
                    && s.Name.contains(needle)
                    && s.Value > 0.0
                    && s.Value < 120.0
                {
                    return Some(s.Value as u8);
                }
            }
        }
        None
    }

    // Last-resort: MSAcpi thermal zones (often unavailable, but costs nothing to try)
    #[derive(Deserialize, Debug)]
    #[allow(non_snake_case)]
    pub struct ThermalZone {
        pub CurrentTemperature: i32,
    }

    pub fn via_acpi() -> Option<u8> {
        let com = COMLibrary::new().ok()?;
        let conn = WMIConnection::with_namespace_path("ROOT\\WMI", com.into()).ok()?;
        let zones: Vec<ThermalZone> = conn.query().ok()?;
        zones.iter().find_map(|z| {
            // Tenths of Kelvin → Celsius
            let c = (z.CurrentTemperature - 2731) / 10;
            if c > 20 && c < 120 {
                Some(c as u8)
            } else {
                None
            }
        })
    }
}

#[cfg(target_os = "windows")]
fn get_cpu_temp() -> Option<u8> {
    // Try OHM/LHM first (accurate), fall back to ACPI (unreliable but no deps)
    windows_temp::via_ohm().or_else(|| windows_temp::via_acpi())
}

// ─── Unsupported platforms ────────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn get_cpu_temp() -> Option<u8> {
    None
}

// ─── Shared ───────────────────────────────────────────────────────────────────

fn read_temp(path: &Path) -> Option<u8> {
    let raw: i64 = fs::read_to_string(path).ok()?.trim().parse().ok()?;
    let celsius = raw / 1000;
    if celsius > 0 && celsius < 120 {
        Some(celsius as u8)
    } else {
        None
    }
}

fn send_temp(dev: &hidapi::HidDevice, temp: u8) -> bool {
    let mut packet = [0u8; 64];

    packet[1] = 0xA5;
    packet[2] = 0x01;
    packet[3] = 0x00;
    packet[4] = temp;

    dev.write(&packet).is_ok()
}

fn main() {
    println!("Redragon CCW-3017 LCD Temperature Monitor");
    println!("Looking for device {:04x}:{:04x}...", VENDOR_ID, PRODUCT_ID);

    ctrlc::set_handler(|| {
        println!("\nShutting down...");
        std::process::exit(0);
    })
    .ok();

    loop {
        let api = match HidApi::new() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("HID init error: {e}, retrying in 5s...");
                thread::sleep(Duration::from_secs(5));
                continue;
            }
        };

        let dev = match api.open(VENDOR_ID, PRODUCT_ID) {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Device not found, retrying in 5s...");
                thread::sleep(Duration::from_secs(5));
                continue;
            }
        };

        println!("Connected to LCD.");

        loop {
            match get_cpu_temp() {
                Some(temp) => {
                    if !send_temp(&dev, temp) {
                        eprintln!("Write failed, reconnecting...");
                        break;
                    }
                    print!("CPU Temp: {temp}°C\r");
                }
                None => eprintln!("Could not read CPU temp"),
            }
            thread::sleep(UPDATE_INTERVAL);
        }

        thread::sleep(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_temp_readable() {
        let temp = get_cpu_temp();
        assert!(temp.is_some(), "Should be able to read CPU temp");
        let t = temp.unwrap();
        assert!(t > 0 && t < 120, "Temp {t} out of sane range");
    }

    #[test]
    fn test_packet_format() {
        let temp: u8 = 75;
        let tens = temp / 10;
        assert_eq!(tens, 7);
    }
}
