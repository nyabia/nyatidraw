//! Per-user alpha associations, touched only by Velopack's fast lifecycle hooks.
use std::{io, path::Path};
use velopack::locator::{LocationContext, auto_locate_app_manifest};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR},
        System::Registry::{
            HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE,
            REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
            RegDeleteKeyW, RegDeleteValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW,
            RegSetValueExW,
        },
        UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify},
    },
    core::PCWSTR,
};

const OWNER: &str = "NyatiDrawAlphaOwner";
const DEFAULT_OWNER: &str = "NyatiDrawAlphaDefaultOwner";
const FORMATS: [(&str, &str, &str); 2] = [
    (".png", "NyatiDraw.Alpha.PNG.1", "NyatiDraw Alpha PNG"),
    (
        ".ntdr",
        "NyatiDraw.Alpha.Project.1",
        "NyatiDraw Alpha Project",
    ),
];

pub(crate) fn register() {
    run(false);
}

pub(crate) fn unregister() {
    run(true);
}

fn run(remove: bool) {
    if let Err(error) = update(remove) {
        eprintln!("native-shell event=file-associations-failed remove={remove} error={error}");
    }
    // Notify even after a partial failure; registration does not affect artwork.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

fn update(remove: bool) -> Result<(), Box<dyn std::error::Error>> {
    let locator = auto_locate_app_manifest(LocationContext::FromCurrentExe)?;
    if locator.get_is_portable() || locator.get_manifest_id() != "NyatiDraw.Alpha" {
        return Ok(());
    }
    let Some(classes) = Key::open(HKEY_CURRENT_USER, "Software\\Classes", !remove)? else {
        return Ok(());
    };
    update_in(
        classes.0,
        HKEY_CLASSES_ROOT,
        &locator.get_root_dir(),
        remove,
    )
}

// Both roots are injected so acceptance never touches real Classes/UserChoice.
#[allow(clippy::too_many_lines)]
fn update_in(
    registry: HKEY,
    effective_registry: HKEY,
    root: &Path,
    remove: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let root_text = path_text(root)?;
    let updater = path_text(&root.join("Update.exe"))?;
    let executable = path_text(&root.join("current/nyatidraw-desktop.exe"))?;
    let command = format!("\"{updater}\" start nyatidraw-desktop.exe -- \"%1\"");
    let icon = format!("\"{executable}\",0");
    for (extension, prog_id, title) in FORMATS {
        let values = [
            ("", "", title),
            ("DefaultIcon", "", icon.as_str()),
            ("Application", "ApplicationName", "NyatiDraw Alpha"),
            ("Application", "ApplicationIcon", icon.as_str()),
            ("Application", "AppUserModelId", "velopack.NyatiDraw.Alpha"),
            ("shell\\open\\command", "", command.as_str()),
        ];
        let existing = Key::open(registry, prog_id, false)?;
        if remove {
            if !existing
                .as_ref()
                .is_some_and(|key| key.matches(OWNER, &root_text).unwrap_or(false))
            {
                continue;
            }
            remove_matching(
                registry,
                &format!("{extension}\\OpenWithProgids"),
                prog_id,
                "",
            )?;
            if extension == ".ntdr"
                && let Some(key) = Key::open(registry, extension, false)?
                && key.matches(DEFAULT_OWNER, &root_text)?
            {
                remove_matching(registry, extension, "", prog_id)?;
                remove_matching(registry, extension, DEFAULT_OWNER, &root_text)?;
            }
            for (suffix, name, value) in values.iter().rev() {
                remove_matching(registry, &subkey(prog_id, suffix), name, value)?;
            }
            remove_matching(registry, prog_id, OWNER, &root_text)?;
            for suffix in [
                "shell\\open\\command",
                "shell\\open",
                "shell",
                "DefaultIcon",
                "Application",
                "",
            ] {
                prune_empty(registry, &subkey(prog_id, suffix))?;
            }
            prune_empty(registry, &format!("{extension}\\OpenWithProgids"))?;
            prune_empty(registry, extension)?;
            continue;
        }
        if let Some(key) = &existing
            && !key.matches(OWNER, &root_text)?
        {
            return Err(io::Error::other(format!("Unowned association: {prog_id}")).into());
        }
        // Preflight every value in this format before modifying it. A changed
        // command is not assumed to belong to us merely because the marker does.
        for (suffix, name, value) in &values {
            assert_available(registry, &subkey(prog_id, suffix), name, value)?;
        }
        let open_with = format!("{extension}\\OpenWithProgids");
        if existing.is_none()
            && let Some(key) = Key::open(registry, &open_with, false)?
            && key.read(prog_id)?.is_some()
        {
            return Err(io::Error::other("Unowned Open With reference").into());
        }
        assert_available(registry, &open_with, prog_id, "")?;
        set(registry, prog_id, OWNER, &root_text)?;
        for (suffix, name, value) in values {
            set(registry, &subkey(prog_id, suffix), name, value)?;
        }
        set(registry, &open_with, prog_id, "")?;
        // Preserve machine/user defaults as well as Explorer's protected
        // UserChoice. PNG is only an Open With candidate, never our default.
        if extension == ".ntdr" {
            let effective = Key::open(effective_registry, extension, false)?;
            let no_default = effective
                .as_ref()
                .map(|key| key.read(""))
                .transpose()?
                .flatten()
                .is_none();
            if no_default {
                assert_available(registry, extension, DEFAULT_OWNER, &root_text)?;
                set(registry, extension, DEFAULT_OWNER, &root_text)?;
                set(registry, extension, "", prog_id)?;
            }
        }
    }
    Ok(())
}

fn path_text(path: &Path) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::other("Non-Unicode install path"))?;
    if text.contains(['\0', '"']) {
        return Err(io::Error::other("Invalid install path"));
    }
    Ok(text.to_owned())
}

fn subkey(root: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        root.to_owned()
    } else {
        format!("{root}\\{suffix}")
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn string_data(text: &str) -> Vec<u8> {
    wide(text).into_iter().flat_map(u16::to_le_bytes).collect()
}

fn check(result: WIN32_ERROR) -> io::Result<()> {
    result.ok().map_err(io::Error::other)
}

// Every handle is owned and closed once. Win32 calls below receive live,
// NUL-terminated UTF-16 buffers and valid output pointers for the call duration;
// reads cap allocation and propagate a concurrent size/type change as an error.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl Key {
    fn open(root: HKEY, path: &str, create: bool) -> io::Result<Option<Self>> {
        let path = wide(path);
        let mut key = HKEY::default();
        let result = unsafe {
            if create {
                RegCreateKeyExW(
                    root,
                    PCWSTR(path.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    None,
                    &raw mut key,
                    None,
                )
            } else {
                RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, KEY_READ, &raw mut key)
            }
        };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(result)?;
        Ok(Some(Self(key)))
    }

    fn read(&self, name: &str) -> io::Result<Option<(REG_VALUE_TYPE, Vec<u8>)>> {
        let name = wide(name);
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0;
        let result = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&raw mut kind),
                None,
                Some(&raw mut size),
            )
        };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(result)?;
        if size > 65536 {
            return Err(io::Error::other("Association value exceeds limit"));
        }
        let mut data = vec![0; size as usize];
        check(unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&raw mut kind),
                Some(data.as_mut_ptr()),
                Some(&raw mut size),
            )
        })?;
        data.truncate(size as usize);
        Ok(Some((kind, data)))
    }

    fn matches(&self, name: &str, expected: &str) -> io::Result<bool> {
        Ok(self.read(name)? == Some((REG_SZ, string_data(expected))))
    }
}

fn assert_available(registry: HKEY, path: &str, name: &str, expected: &str) -> io::Result<()> {
    if let Some(key) = Key::open(registry, path, false)?
        && let Some(actual) = key.read(name)?
        && actual != (REG_SZ, string_data(expected))
    {
        return Err(io::Error::other(format!(
            "Association value collision: {path}/{name}"
        )));
    }
    Ok(())
}

fn set(registry: HKEY, path: &str, name: &str, value: &str) -> io::Result<()> {
    let key = Key::open(registry, path, true)?
        .ok_or_else(|| io::Error::other("Cannot create association key"))?;
    let name = wide(name);
    check(unsafe {
        RegSetValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            REG_SZ,
            Some(&string_data(value)),
        )
    })
}

fn remove_matching(registry: HKEY, path: &str, name: &str, expected: &str) -> io::Result<()> {
    if let Some(key) = Key::open(registry, path, false)?
        && key.matches(name, expected)?
    {
        let writable = Key::open(registry, path, true)?
            .ok_or_else(|| io::Error::other("Association key disappeared"))?;
        let name = wide(name);
        check(unsafe { RegDeleteValueW(writable.0, PCWSTR(name.as_ptr())) })?;
    }
    Ok(())
}

fn prune_empty(registry: HKEY, path: &str) -> io::Result<()> {
    if let Some(key) = Key::open(registry, path, false)? {
        let (mut subkeys, mut values) = (0, 0);
        check(unsafe {
            RegQueryInfoKeyW(
                key.0,
                None,
                None,
                None,
                Some(&raw mut subkeys),
                None,
                None,
                Some(&raw mut values),
                None,
                None,
                None,
                None,
            )
        })?;
        if subkeys == 0 && values == 0 {
            drop(key);
            check(unsafe { RegDeleteKeyW(registry, PCWSTR(wide(path).as_ptr())) })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod acceptance {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows::Win32::System::Registry::RegDeleteTreeW;

    struct Scratch(String);

    impl Drop for Scratch {
        fn drop(&mut self) {
            // Only the fresh nonce subtree created by this test is removed.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(&self.0).as_ptr()));
            }
        }
    }

    // Deliberate OS acceptance, not a mock suite or an ordinary CI unit test.
    #[test]
    #[ignore = "explicit Windows scratch-registry acceptance; never edits Classes"]
    #[allow(clippy::too_many_lines)]
    fn lifecycle_preserves_foreign_associations_and_user_choices() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = format!(
            "Software\\NyatiDraw\\Acceptance\\Associations-{}-{nonce}",
            std::process::id()
        );
        assert!(
            Key::open(HKEY_CURRENT_USER, &path, false)
                .unwrap()
                .is_none()
        );
        let scratch = Scratch(path);
        let scratch_key = Key::open(HKEY_CURRENT_USER, &scratch.0, true)
            .unwrap()
            .unwrap();
        let install = Path::new(r"C:\NyatiDraw acceptance 한글\alpha");
        for existing_project_default in [false, true] {
            let classes = Key::open(scratch_key.0, &existing_project_default.to_string(), true)
                .unwrap()
                .unwrap();
            let registry = classes.0;
            set(registry, ".png", "", "Foreign.PNG").unwrap();
            set(registry, ".png\\UserChoice", "ProgId", "Foreign.PNG").unwrap();
            set(registry, ".png\\OpenWithProgids", "Foreign.PNG", "").unwrap();
            if existing_project_default {
                set(registry, ".ntdr", "", "Foreign.Project").unwrap();
            }
            update_in(registry, registry, install, false).unwrap();
            update_in(registry, registry, install, false).unwrap();
            for (extension, prog_id, _) in FORMATS {
                let key = Key::open(registry, &format!("{extension}\\OpenWithProgids"), false)
                    .unwrap()
                    .unwrap();
                assert!(key.matches(prog_id, "").unwrap());
            }
            let project = Key::open(registry, ".ntdr", false).unwrap().unwrap();
            assert!(
                project
                    .matches(
                        "",
                        if existing_project_default {
                            "Foreign.Project"
                        } else {
                            FORMATS[1].1
                        }
                    )
                    .unwrap()
            );
            drop(project);
            // Updating must refuse a hijacked command. Uninstall may remove
            // only our unchanged values, retaining the foreign command/subkey.
            let command = format!("{}\\shell\\open\\command", FORMATS[0].1);
            set(registry, &command, "", "foreign command").unwrap();
            set(
                registry,
                &format!("{}\\Foreign", FORMATS[0].1),
                "Data",
                "keep",
            )
            .unwrap();
            assert!(update_in(registry, registry, install, false).is_err());
            update_in(registry, registry, install, true).unwrap();
            update_in(registry, registry, install, true).unwrap();
            for (key, name, value) in [
                (".png", "", "Foreign.PNG"),
                (".png\\UserChoice", "ProgId", "Foreign.PNG"),
                (".png\\OpenWithProgids", "Foreign.PNG", ""),
                (&command, "", "foreign command"),
                (&format!("{}\\Foreign", FORMATS[0].1), "Data", "keep"),
            ] {
                assert!(
                    Key::open(registry, key, false)
                        .unwrap()
                        .unwrap()
                        .matches(name, value)
                        .unwrap()
                );
            }
            assert!(Key::open(registry, FORMATS[1].1, false).unwrap().is_none());
            if existing_project_default {
                assert!(
                    Key::open(registry, ".ntdr", false)
                        .unwrap()
                        .unwrap()
                        .matches("", "Foreign.Project")
                        .unwrap()
                );
            } else {
                assert!(Key::open(registry, ".ntdr", false).unwrap().is_none());
            }
            assert!(
                Key::open(registry, ".png\\OpenWithProgids", false)
                    .unwrap()
                    .unwrap()
                    .read(FORMATS[0].1)
                    .unwrap()
                    .is_none()
            );
        }
        // Matching strings without an owner must not be adopted or deleted.
        let collision = Key::open(scratch_key.0, "collision", true)
            .unwrap()
            .unwrap();
        set(collision.0, FORMATS[0].1, "", "foreign registration").unwrap();
        assert!(update_in(collision.0, collision.0, install, false).is_err());
        update_in(collision.0, collision.0, install, true).unwrap();
        assert!(
            Key::open(collision.0, FORMATS[0].1, false)
                .unwrap()
                .unwrap()
                .matches("", "foreign registration")
                .unwrap()
        );
    }
}
