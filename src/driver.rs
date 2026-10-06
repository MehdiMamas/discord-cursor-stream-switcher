//! Installs, configures and removes the Virtual Display Driver (VirtualDrivers/Virtual-Display-Driver,
//! MIT) that provides the "Stream Cursor" screen.
//!
//! The signed driver files (release 25.7.23, signed by SignPath Foundation) are embedded in
//! this .exe and pinned by SHA-256 in tests, so installing needs no download. Windows checks the
//! driver signature itself and asks the user before installing it; this code never adds
//! certificates to the machine's trusted stores.
//!
//! These functions run in a separate elevated copy of the app (`driver-install` etc.).

use crate::paths;
use std::path::{Path, PathBuf};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, HLOCAL, HWND, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
    SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

pub const DRIVER_VERSION: &str = "25.7.23";
const HARDWARE_ID: &str = r"Root\MttVDD";
/// The driver reads its settings from here (hard-coded in the driver).
pub const SETTINGS_DIR: &str = r"C:\VirtualDisplayDriver";
const MARKER: &str = "Managed by Cursor Stream Switcher";

const INF: &[u8] = include_bytes!("../vendor/vdd/MttVDD.inf");
const DLL: &[u8] = include_bytes!("../vendor/vdd/MttVDD.dll");
const CAT: &[u8] = include_bytes!("../vendor/vdd/mttvdd.cat");

/// The name Windows shows for the virtual monitor (EDID names are at most 13 characters).
pub const MONITOR_NAME: &str = "Stream Cursor";

/// Resolutions offered in the tray menu.
pub const RESOLUTIONS: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
pub const DEFAULT_MODE: (u32, u32, u32) = (1920, 1080, 60);

/// The driver's built-in EDID (from Driver.cpp, MIT). Manufacturer "MTT", product 0x1337.
const BASE_EDID: [u8; 256] = [
    0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x36, 0x94, 0x37, 0x13, 0xe7, 0x1e, 0xe7, 0x1e, 0x1c,
    0x22, 0x01, 0x03, 0x80, 0x32, 0x1f, 0x78, 0x07, 0xee, 0x95, 0xa3, 0x54, 0x4c, 0x99, 0x26, 0x0f, 0x50,
    0x54, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c, 0x45, 0x00, 0x63, 0xc8,
    0x10, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0xfd, 0x00, 0x17, 0xf0, 0x0f, 0xff, 0x37, 0x00, 0x0a, 0x20,
    0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xfc, 0x00, 0x56, 0x44, 0x44, 0x20, 0x62, 0x79,
    0x20, 0x4d, 0x54, 0x54, 0x0a, 0x20, 0x20, 0x01, 0xc2, 0x02, 0x03, 0x20, 0x40, 0xe6, 0x06, 0x0d, 0x01,
    0xa2, 0xa2, 0x10, 0xe3, 0x05, 0xd8, 0x00, 0x67, 0xd8, 0x5d, 0xc4, 0x01, 0x6e, 0x80, 0x00, 0x68, 0x03,
    0x0c, 0x00, 0x00, 0x00, 0x30, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x8c,
];

/// The built-in EDID with the monitor name descriptor replaced and checksums fixed.
pub fn edid_with_name(name: &str) -> [u8; 256] {
    let mut edid = BASE_EDID;
    // Detailed descriptors live at 54, 72, 90 and 108; the name one has tag 0xFC.
    for start in [54usize, 72, 90, 108] {
        if edid[start..start + 3] == [0, 0, 0] && edid[start + 3] == 0xfc {
            let text = &mut edid[start + 5..start + 18];
            text.fill(0x20);
            let bytes: Vec<u8> = name.bytes().filter(u8::is_ascii_graphic_or_space).take(13).collect();
            text[..bytes.len()].copy_from_slice(&bytes);
            if bytes.len() < 13 {
                text[bytes.len()] = 0x0a;
            }
        }
    }
    for block in 0..2 {
        let b = &mut edid[block * 128..block * 128 + 128];
        let sum = b[..127].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b[127] = 0u8.wrapping_sub(sum);
    }
    edid
}

trait AsciiName {
    fn is_ascii_graphic_or_space(&self) -> bool;
}
impl AsciiName for u8 {
    fn is_ascii_graphic_or_space(&self) -> bool {
        self.is_ascii_graphic() || *self == b' '
    }
}

/// vdd_settings.xml with exactly one virtual monitor and one display mode, so Windows can't
/// pick anything else.
pub fn settings_xml(width: u32, height: u32, refresh: u32) -> String {
    format!(
        r#"<?xml version='1.0' encoding='utf-8'?>
<!-- {MARKER}. Change the resolution from its tray menu (Virtual display). -->
<vdd_settings>
    <monitors>
        <count>1</count>
    </monitors>
    <gpu>
        <friendlyname>default</friendlyname>
    </gpu>
    <global>
        <g_refresh_rate>{refresh}</g_refresh_rate>
    </global>
    <resolutions>
        <resolution>
            <width>{width}</width>
            <height>{height}</height>
            <refresh_rate>{refresh}</refresh_rate>
        </resolution>
    </resolutions>
    <options>
        <CustomEdid>true</CustomEdid>
        <PreventSpoof>false</PreventSpoof>
        <EdidCeaOverride>false</EdidCeaOverride>
        <HardwareCursor>true</HardwareCursor>
        <SDR10bit>false</SDR10bit>
        <HDRPlus>false</HDRPlus>
        <logging>false</logging>
        <debuglogging>false</debuglogging>
    </options>
</vdd_settings>
"#
    )
}

/// Reads width/height/refresh back from our settings file, if we wrote it.
pub fn current_mode() -> Option<(u32, u32, u32)> {
    let text = std::fs::read_to_string(Path::new(SETTINGS_DIR).join("vdd_settings.xml")).ok()?;
    if !text.contains(MARKER) {
        return None;
    }
    let tag = |name: &str| -> Option<u32> {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let end = start + text[start..].find('<')?;
        text[start..end].trim().parse().ok()
    };
    Some((tag("width")?, tag("height")?, tag("refresh_rate")?))
}

pub fn parse_mode(args: &[String]) -> Result<(u32, u32, u32), String> {
    let (mut w, mut h, mut r) = DEFAULT_MODE;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let n: u32 = value.parse().map_err(|_| format!("{flag}: not a number: {value}"))?;
        match flag.as_str() {
            "--width" => w = n,
            "--height" => h = n,
            "--refresh" => r = n,
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if !(640..=7680).contains(&w) || !(480..=4320).contains(&h) || !(24..=240).contains(&r) {
        return Err(format!("unsupported mode {w}x{h}@{r}"));
    }
    Ok((w, h, r))
}

/// Exit codes of the elevated commands.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_REBOOT: i32 = 2;

/// Entry point for `driver-install`, `driver-configure` and `driver-uninstall`.
pub fn cli(command: &str, args: &[String]) -> i32 {
    let result = match command {
        "driver-install" => parse_mode(args).and_then(|(w, h, r)| install(w, h, r)),
        "driver-configure" => parse_mode(args).and_then(|(w, h, r)| configure(w, h, r)),
        "driver-uninstall" => uninstall(),
        _ => Err(format!("unknown command {command}")),
    };
    match result {
        Ok(reboot) => {
            crate::info!("{command} finished{}", if reboot { "; restart needed" } else { "" });
            if reboot { EXIT_REBOOT } else { EXIT_OK }
        }
        Err(e) => {
            crate::error!("{command} failed: {e}");
            EXIT_FAILED
        }
    }
}

/// Owns an HDEVINFO and destroys it on drop.
struct DevInfo(HDEVINFO);
impl Drop for DevInfo {
    fn drop(&mut self) {
        // SAFETY: the set was created by SetupDi* and is destroyed once.
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn wide_multi_sz(bytes: &[u8]) -> Vec<String> {
    let words: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    words.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

/// All display-class devices (present or not) whose hardware IDs include ours.
fn our_devices() -> Result<(DevInfo, Vec<SP_DEVINFO_DATA>), String> {
    // SAFETY: SetupAPI enumeration with correctly sized structs and buffers.
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_DISPLAY),
            PCWSTR::null(),
            None,
            SETUP_DI_GET_CLASS_DEVS_FLAGS(0),
        )
        .map_err(|e| format!("SetupDiGetClassDevs: {e}"))?;
        let set = DevInfo(set);
        let mut found = Vec::new();
        for i in 0.. {
            let mut data = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            match SetupDiEnumDeviceInfo(set.0, i, &mut data) {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() => break,
                Err(e) => return Err(format!("SetupDiEnumDeviceInfo: {e}")),
            }
            let mut buf = vec![0u8; 2048];
            let mut needed = 0u32;
            if SetupDiGetDeviceRegistryPropertyW(
                set.0,
                &data,
                SPDRP_HARDWAREID,
                None,
                Some(&mut buf),
                Some(&mut needed),
            )
            .is_ok()
                && wide_multi_sz(&buf[..(needed as usize).min(buf.len())])
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(HARDWARE_ID))
            {
                found.push(data);
            }
        }
        Ok((set, found))
    }
}

/// True if the driver's device node exists (even if the display is currently switched off).
pub fn is_installed() -> bool {
    our_devices().is_ok_and(|(_, d)| !d.is_empty())
}

/// Lets SYSTEM and Administrators change a folder, everyone else only read it. The driver runs
/// as a low-privilege service and only needs to read its settings.
fn lock_down(dir: &Path) -> Result<(), String> {
    let sddl = w!("D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;WD)");
    let path = HSTRING::from(dir.as_os_str());
    // SAFETY: the security descriptor is freed with LocalFree; the DACL points into it.
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None)
            .map_err(|e| format!("bad SDDL: {e}"))?;
        let mut present = BOOL(0);
        let mut defaulted = BOOL(0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let r = GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted)
            .map_err(|e| format!("GetSecurityDescriptorDacl: {e}"))
            .and_then(|_| {
                let err = SetNamedSecurityInfoW(
                    &path,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    Some(dacl),
                    None,
                );
                if err.is_ok() {
                    Ok(())
                } else {
                    Err(format!("can't set permissions on {}: {err:?}", dir.display()))
                }
            });
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        r
    }
}

fn write_settings(width: u32, height: u32, refresh: u32) -> Result<(), String> {
    let dir = PathBuf::from(SETTINGS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    lock_down(&dir)?;
    let settings = dir.join("vdd_settings.xml");
    if let Ok(existing) = std::fs::read_to_string(&settings)
        && !existing.contains(MARKER)
    {
        let backup = dir.join("vdd_settings.xml.before-cursor-stream-switcher");
        if !backup.exists() {
            std::fs::copy(&settings, &backup).map_err(|e| format!("can't back up settings: {e}"))?;
            crate::info!("backed up existing driver settings to {}", backup.display());
        }
    }
    std::fs::write(&settings, settings_xml(width, height, refresh))
        .map_err(|e| format!("can't write settings: {e}"))?;
    std::fs::write(dir.join("user_edid.bin"), edid_with_name(MONITOR_NAME))
        .map_err(|e| format!("can't write EDID: {e}"))?;
    Ok(())
}

fn extract_driver() -> Result<PathBuf, String> {
    let root = paths::machine_dir();
    std::fs::create_dir_all(&root).map_err(|e| format!("can't create {}: {e}", root.display()))?;
    lock_down(&root)?;
    let dir = root.join("driver").join(DRIVER_VERSION);
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    for (name, bytes) in [("MttVDD.inf", INF), ("MttVDD.dll", DLL), ("mttvdd.cat", CAT)] {
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("can't write {name}: {e}"))?;
    }
    Ok(dir.join("MttVDD.inf"))
}

fn install(width: u32, height: u32, refresh: u32) -> Result<bool, String> {
    crate::info!("installing Virtual Display Driver {DRIVER_VERSION} at {width}x{height}@{refresh}");
    write_settings(width, height, refresh)?;
    let inf = extract_driver()?;
    let inf_w = HSTRING::from(inf.as_os_str());
    let hwid = HSTRING::from(HARDWARE_ID);

    let (_, existing) = our_devices()?;
    let mut created: Option<(DevInfo, SP_DEVINFO_DATA)> = None;
    if existing.is_empty() {
        // Create the root-enumerated device node the driver binds to (what `devcon install` does).
        // SAFETY: SetupAPI calls with valid, correctly sized structs.
        unsafe {
            let set = DevInfo(
                SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_DISPLAY), None)
                    .map_err(|e| format!("SetupDiCreateDeviceInfoList: {e}"))?,
            );
            let mut data = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            SetupDiCreateDeviceInfoW(
                set.0,
                w!("Display"),
                &GUID_DEVCLASS_DISPLAY,
                PCWSTR::null(),
                None,
                DICD_GENERATE_ID,
                Some(&mut data),
            )
            .map_err(|e| format!("SetupDiCreateDeviceInfo: {e}"))?;
            let mut ids: Vec<u8> =
                HARDWARE_ID.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes).collect();
            ids.shrink_to_fit();
            SetupDiSetDeviceRegistryPropertyW(set.0, &mut data, SPDRP_HARDWAREID, Some(&ids))
                .map_err(|e| format!("set hardware ID: {e}"))?;
            SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, Some(&data))
                .map_err(|e| format!("register device: {e}"))?;
            created = Some((set, data));
        }
    }

    let mut reboot = BOOL(0);
    // Windows verifies the driver signature here and asks the user to confirm the publisher.
    // SAFETY: valid strings; reboot flag is a plain out-param.
    let r = unsafe {
        UpdateDriverForPlugAndPlayDevicesW(None::<HWND>, &hwid, &inf_w, INSTALLFLAG_FORCE, Some(&mut reboot))
    };
    if let Err(e) = r {
        if let Some((set, data)) = &created {
            // Don't leave an empty device node behind.
            // SAFETY: removing the device we registered above.
            unsafe {
                let _ = SetupDiCallClassInstaller(DIF_REMOVE, set.0, Some(data));
            }
        }
        return Err(format!("driver install failed or was cancelled: {e}"));
    }
    if !existing.is_empty() {
        restart_devices()?;
    }
    Ok(reboot.as_bool())
}

fn restart_devices() -> Result<(), String> {
    let (set, devices) = our_devices()?;
    for data in devices {
        let params = SP_PROPCHANGE_PARAMS {
            ClassInstallHeader: SP_CLASSINSTALL_HEADER {
                cbSize: std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as u32,
                InstallFunction: DIF_PROPERTYCHANGE,
            },
            StateChange: DICS_PROPCHANGE,
            Scope: DICS_FLAG_GLOBAL,
            HwProfile: 0,
        };
        // SAFETY: params is a valid SP_PROPCHANGE_PARAMS starting with its class install header.
        unsafe {
            SetupDiSetClassInstallParamsW(
                set.0,
                Some(&data),
                Some(&params.ClassInstallHeader),
                std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as u32,
            )
            .map_err(|e| format!("SetupDiSetClassInstallParams: {e}"))?;
            SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set.0, Some(&data))
                .map_err(|e| format!("restart device: {e}"))?;
        }
    }
    Ok(())
}

/// Rewrites the settings and restarts the driver so the new resolution applies.
fn configure(width: u32, height: u32, refresh: u32) -> Result<bool, String> {
    crate::info!("setting virtual display to {width}x{height}@{refresh}");
    write_settings(width, height, refresh)?;
    restart_devices()?;
    Ok(false)
}

fn uninstall() -> Result<bool, String> {
    crate::info!("uninstalling Virtual Display Driver");
    let mut reboot = false;
    let (set, devices) = our_devices()?;
    for data in &devices {
        let mut need = BOOL(0);
        // SAFETY: removing a device from a set we enumerated.
        unsafe { DiUninstallDevice(HWND::default(), set.0, data, 0, Some(&mut need)) }
            .map_err(|e| format!("remove device: {e}"))?;
        reboot |= need.as_bool();
    }
    drop(set);

    // Remove the driver package from the driver store: find the oem*.inf that came from MttVDD.inf.
    let inf_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("INF");
    if let Ok(entries) = std::fs::read_dir(&inf_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !(name.starts_with("oem") && name.ends_with(".inf")) {
                continue;
            }
            let text = std::fs::read(entry.path())
                .map(|b| String::from_utf8_lossy(&b).to_ascii_lowercase())
                .unwrap_or_default();
            if text.contains("catalogfile=mttvdd.cat") && text.contains(r"root\mttvdd") {
                let wname = HSTRING::from(entry.file_name());
                // SAFETY: valid file name; flags request removal even if devices still reference it.
                let ok = unsafe { SetupUninstallOEMInfW(&wname, SUOI_FORCEDELETE, None) };
                crate::info!("removed driver package {} ({})", name, ok.as_bool());
            }
        }
    }

    // Remove the settings we wrote; leave the folder if anything else is in it.
    let dir = Path::new(SETTINGS_DIR);
    if std::fs::read_to_string(dir.join("vdd_settings.xml")).is_ok_and(|t| t.contains(MARKER)) {
        let _ = std::fs::remove_file(dir.join("vdd_settings.xml"));
        let _ = std::fs::remove_file(dir.join("user_edid.bin"));
        let _ = std::fs::remove_dir(dir);
    }
    let _ = std::fs::remove_dir_all(paths::machine_dir().join("driver"));
    Ok(reboot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn embedded_driver_files_are_the_pinned_release() {
        // SHA-256 of VirtualDisplayDriver-x86.Driver.Only.zip contents from release 25.7.23
        // (the x64 driver despite the asset name). Signed by SignPath Foundation.
        assert_eq!(sha256(INF), "550d211fe481e74dfe3f9d724ed78be48b3a9113405965d683d9373e8d672f5d");
        assert_eq!(sha256(DLL), "c9ca837f57a98fbd43bc416a7f535a95843626e7759eaf85cf0cd7ce334dbb05");
        assert_eq!(sha256(CAT), "08a0093fc9b2e32b287a6f8a77ca4de0a31830d29fc33d2b13a918dc859468f6");
    }

    #[test]
    fn edid_name_and_checksums() {
        let edid = edid_with_name(MONITOR_NAME);
        assert_eq!(&edid[113..126], b"Stream Cursor");
        for block in edid.chunks(128) {
            assert_eq!(block.iter().fold(0u8, |a, &x| a.wrapping_add(x)), 0);
        }
        // Manufacturer/product (MTT1337) untouched so the app can still find the display.
        assert_eq!(&edid[8..12], &BASE_EDID[8..12]);
        // The unmodified base EDID is valid too.
        assert_eq!(BASE_EDID[..128].iter().fold(0u8, |a, &x| a.wrapping_add(x)), 0);
        // Short names are terminated with a newline and padded.
        let short = edid_with_name("Stream");
        assert_eq!(&short[113..126], b"Stream\n      ");
    }

    #[test]
    fn settings_have_one_mode() {
        let xml = settings_xml(2560, 1440, 60);
        assert_eq!(xml.matches("<resolution>").count(), 1);
        assert!(xml.contains("<width>2560</width>") && xml.contains("<refresh_rate>60</refresh_rate>"));
        assert!(xml.contains("<count>1</count>") && xml.contains("<CustomEdid>true</CustomEdid>"));
        assert!(xml.contains(MARKER));
    }

    #[test]
    fn mode_arguments() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_mode(&[]), Ok(DEFAULT_MODE));
        assert_eq!(parse_mode(&s(&["--width", "2560", "--height", "1440"])), Ok((2560, 1440, 60)));
        assert!(parse_mode(&s(&["--width"])).is_err());
        assert!(parse_mode(&s(&["--width", "99999"])).is_err());
        assert!(parse_mode(&s(&["--fps", "60"])).is_err());
    }

    #[test]
    fn multi_sz_parsing() {
        let bytes: Vec<u8> = "Root\\MttVDD\0MttVDD\0\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(wide_multi_sz(&bytes), vec!["Root\\MttVDD".to_owned(), "MttVDD".to_owned()]);
    }
}
