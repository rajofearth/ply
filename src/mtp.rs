//! Browsing phones and cameras over MTP, via Windows Portable Devices.
//!
//! These devices have no drive letter and no filesystem path: WPD addresses
//! everything by opaque per-session object IDs. To let MTP objects travel
//! through the rest of Ply — which is built on [`PathBuf`] — each object gets a
//! virtual path:
//!
//! ```text
//! \\MTP\<device-key>\<object-id>\<object-id>...
//! ```
//!
//! Windows parses `\\MTP\<device-key>` as a UNC prefix, so `Path::ancestors`
//! walks back up the object chain and breadcrumbs work unchanged. The device
//! key is a hash of the Plug and Play ID rather than the ID itself, because
//! those contain backslashes and would otherwise split into path components.
//!
//! All COM work happens on one dedicated thread that keeps its own apartment
//! and caches open device handles, since opening a device costs far more than
//! any single listing.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{LazyLock, Mutex};

use crate::listing::Entry;

/// A connected portable device, reported as a volume under Devices & network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub key: String,
    pub name: String,
    pub free: u64,
    pub total: u64,
}

impl Device {
    pub fn root(&self) -> PathBuf {
        root_of(&self.key)
    }
}

/// Plug and Play IDs by device key, so a virtual path can be resolved back to
/// the device it names. Refreshed by every [`devices`] call.
static REGISTRY: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// FNV-1a, chosen over `DefaultHasher` because the key must stay identical for
/// the same device across runs.
fn key_for(pnp_id: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in pnp_id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn root_of(key: &str) -> PathBuf {
    PathBuf::from(format!(r"\\MTP\{key}"))
}

/// Split a virtual path into its device key and the object it names. The
/// device root itself addresses WPD's synthetic `DEVICE` object, whose children
/// are the storages.
pub fn parse(path: &Path) -> Option<(String, String)> {
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    let Prefix::UNC(host, key) = prefix.kind() else {
        return None;
    };
    if !host.eq_ignore_ascii_case("MTP") {
        return None;
    }
    let object = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .next_back()
        .unwrap_or_else(|| "DEVICE".to_string());
    Some((key.to_string_lossy().into_owned(), object))
}

pub fn is_mtp(path: &Path) -> bool {
    parse(path).is_some()
}

fn pnp_id_for(key: &str) -> Option<String> {
    REGISTRY.lock().ok()?.get(key).cloned()
}

#[cfg(windows)]
pub use windows_impl::{devices, fetch, list};

#[cfg(not(windows))]
pub fn devices() -> Vec<Device> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn list(_path: &Path) -> anyhow::Result<Vec<Entry>> {
    anyhow::bail!("Portable devices are only supported on Windows.")
}

#[cfg(not(windows))]
pub fn fetch(_path: &Path) -> anyhow::Result<PathBuf> {
    anyhow::bail!("Portable devices are only supported on Windows.")
}

#[cfg(windows)]
mod windows_impl {
    use super::*;

    use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use anyhow::{Result, anyhow, bail};
    use windows::Win32::Devices::PortableDevices::{
        IEnumPortableDeviceObjectIDs, IPortableDevice, IPortableDeviceContent,
        IPortableDeviceKeyCollection, IPortableDeviceManager, IPortableDeviceProperties,
        IPortableDeviceValues,
    };
    use windows::Win32::Foundation::{FILETIME, PROPERTYKEY};
    use windows::Win32::System::Com::StructuredStorage::PropVariantToFileTime;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, IStream, STGM_READ,
    };
    use windows::Win32::System::Variant::PSTF_UTC;
    use windows::core::{GUID, HSTRING, PWSTR};

    use crate::listing::EntryKind;

    const fn pkey(fmtid: u128, pid: u32) -> PROPERTYKEY {
        PROPERTYKEY {
            fmtid: GUID::from_u128(fmtid),
            pid,
        }
    }

    const OBJECT_SET: u128 = 0xEF6B490D_5CD8_437A_AFFC_DA8B60EE4A3C;
    const CLIENT_SET: u128 = 0x204D9F0C_2292_4080_9F42_40664E70F859;
    const STORAGE_SET: u128 = 0x01A3057A_74D6_4E80_BEA7_DC4C212CE50A;
    const RESOURCE_SET: u128 = 0xE81E79BE_34F0_41BF_B53F_F1A06AE87842;

    const OBJECT_NAME: PROPERTYKEY = pkey(OBJECT_SET, 4);
    const OBJECT_CONTENT_TYPE: PROPERTYKEY = pkey(OBJECT_SET, 7);
    const OBJECT_IS_HIDDEN: PROPERTYKEY = pkey(OBJECT_SET, 9);
    const OBJECT_SIZE: PROPERTYKEY = pkey(OBJECT_SET, 11);
    const OBJECT_ORIGINAL_FILE_NAME: PROPERTYKEY = pkey(OBJECT_SET, 12);
    const OBJECT_DATE_MODIFIED: PROPERTYKEY = pkey(OBJECT_SET, 19);

    const CLIENT_NAME: PROPERTYKEY = pkey(CLIENT_SET, 2);
    const CLIENT_MAJOR: PROPERTYKEY = pkey(CLIENT_SET, 3);
    const CLIENT_MINOR: PROPERTYKEY = pkey(CLIENT_SET, 4);
    const CLIENT_REVISION: PROPERTYKEY = pkey(CLIENT_SET, 5);

    const STORAGE_CAPACITY: PROPERTYKEY = pkey(STORAGE_SET, 4);
    const STORAGE_FREE: PROPERTYKEY = pkey(STORAGE_SET, 5);
    const RESOURCE_DEFAULT: PROPERTYKEY = pkey(RESOURCE_SET, 0);

    /// Folders and functional objects (the storages) are both containers.
    const CONTENT_FOLDER: GUID = GUID::from_u128(0x27E2E392_A111_48E0_AB0C_E17705A05F85);
    const CONTENT_FUNCTIONAL: GUID = GUID::from_u128(0x99ED0160_17FF_4C44_9D98_1D7A6F941921);

    const CLSID_MANAGER: GUID = GUID::from_u128(0x0AF10CEC_2ECD_4B92_9581_34F6AE0637F3);
    const CLSID_DEVICE_FTM: GUID = GUID::from_u128(0xF7C0039A_4762_488A_B4B3_760EF9A1BA9B);
    const CLSID_VALUES: GUID = GUID::from_u128(0x0C15D503_D017_47CE_9016_7B3F978721CC);
    const CLSID_KEY_COLLECTION: GUID = GUID::from_u128(0xDE2D022D_2480_43BE_97F0_D1FA2CF98F4F);

    /// WPD's synthetic root object; its children are the device's storages.
    const DEVICE_OBJECT: &str = "DEVICE";

    enum Request {
        Devices(Sender<Vec<Device>>),
        List(PathBuf, Sender<Result<Vec<Entry>>>),
        Fetch(PathBuf, Sender<Result<PathBuf>>),
    }

    /// Every COM call is funnelled to one thread, so the interfaces below never
    /// cross an apartment boundary and open handles can be reused.
    static WORKER: LazyLock<SyncSender<Request>> = LazyLock::new(|| {
        let (tx, rx) = sync_channel::<Request>(8);
        std::thread::Builder::new()
            .name("ply-mtp".into())
            .spawn(move || worker(rx))
            .ok();
        tx
    });

    /// Blocks until the worker answers. Callers already run on Ply's background
    /// executor, never on the UI thread.
    fn ask<T>(build: impl FnOnce(Sender<T>) -> Request) -> Option<T> {
        let (tx, rx) = channel();
        WORKER.send(build(tx)).ok()?;
        rx.recv().ok()
    }

    pub fn devices() -> Vec<Device> {
        ask(Request::Devices).unwrap_or_default()
    }

    pub fn list(path: &Path) -> Result<Vec<Entry>> {
        ask(|tx| Request::List(path.to_path_buf(), tx))
            .unwrap_or_else(|| Err(anyhow!("The portable device worker stopped.")))
    }

    /// Copy an object into the temp directory so the OS can open it; MTP data
    /// is not reachable through a path.
    pub fn fetch(path: &Path) -> Result<PathBuf> {
        ask(|tx| Request::Fetch(path.to_path_buf(), tx))
            .unwrap_or_else(|| Err(anyhow!("The portable device worker stopped.")))
    }

    struct Opened {
        _device: IPortableDevice,
        content: IPortableDeviceContent,
        properties: IPortableDeviceProperties,
    }

    fn worker(rx: Receiver<Request>) {
        // SAFETY: initialising this thread's apartment before any COM call.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut open: HashMap<String, Opened> = HashMap::new();

        while let Ok(request) = rx.recv() {
            match request {
                Request::Devices(reply) => {
                    let found = unsafe { enumerate(&mut open) };
                    let _ = reply.send(found);
                }
                Request::List(path, reply) => {
                    let result = unsafe { list_children(&mut open, &path) };
                    let _ = reply.send(result);
                }
                Request::Fetch(path, reply) => {
                    let result = unsafe { copy_to_temp(&mut open, &path) };
                    let _ = reply.send(result);
                }
            }
        }
    }

    /// Take ownership of a string WPD allocated for us.
    unsafe fn take_string(raw: PWSTR) -> String {
        if raw.is_null() {
            return String::new();
        }
        // SAFETY: WPD hands back NUL-terminated task-allocated strings.
        unsafe {
            let text = raw.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw.as_ptr() as *const _));
            text
        }
    }

    unsafe fn open_device(pnp_id: &str) -> windows::core::Result<Opened> {
        // SAFETY: standard WPD open sequence; all arguments outlive the calls.
        unsafe {
            let device: IPortableDevice =
                CoCreateInstance(&CLSID_DEVICE_FTM, None, CLSCTX_INPROC_SERVER)?;
            let info: IPortableDeviceValues =
                CoCreateInstance(&CLSID_VALUES, None, CLSCTX_INPROC_SERVER)?;

            info.SetStringValue(&CLIENT_NAME, &HSTRING::from("Ply"))?;
            info.SetUnsignedIntegerValue(&CLIENT_MAJOR, 1)?;
            info.SetUnsignedIntegerValue(&CLIENT_MINOR, 0)?;
            info.SetUnsignedIntegerValue(&CLIENT_REVISION, 0)?;

            device.Open(&HSTRING::from(pnp_id), &info)?;
            let content = device.Content()?;
            let properties = content.Properties()?;
            Ok(Opened {
                _device: device,
                content,
                properties,
            })
        }
    }

    /// Resolve a device key to an open handle, opening it on first use.
    unsafe fn handle<'a>(cache: &'a mut HashMap<String, Opened>, key: &str) -> Result<&'a Opened> {
        let pnp_id =
            pnp_id_for(key).ok_or_else(|| anyhow!("That device is no longer connected."))?;
        if !cache.contains_key(key) {
            // SAFETY: worker thread owns the apartment these handles live in.
            let opened = unsafe { open_device(&pnp_id) }
                .map_err(|e| anyhow!("Could not open the device: {e}"))?;
            cache.insert(key.to_string(), opened);
        }
        Ok(&cache[key])
    }

    unsafe fn property_keys(
        wanted: &[PROPERTYKEY],
    ) -> windows::core::Result<IPortableDeviceKeyCollection> {
        // SAFETY: freshly created collection, keys are static constants.
        unsafe {
            let keys: IPortableDeviceKeyCollection =
                CoCreateInstance(&CLSID_KEY_COLLECTION, None, CLSCTX_INPROC_SERVER)?;
            for key in wanted {
                keys.Add(key)?;
            }
            Ok(keys)
        }
    }

    unsafe fn child_ids(open: &Opened, parent: &str) -> windows::core::Result<Vec<String>> {
        // SAFETY: enumerator is used and dropped on this thread only.
        unsafe {
            let enumerator: IEnumPortableDeviceObjectIDs =
                open.content.EnumObjects(0, &HSTRING::from(parent), None)?;
            let mut ids = Vec::new();
            loop {
                let mut batch = [PWSTR::null(); 32];
                let mut fetched = 0u32;
                // Returns S_FALSE once drained, so stop on the count.
                if enumerator.Next(&mut batch, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                for raw in batch.iter().take(fetched as usize) {
                    ids.push(take_string(*raw));
                }
            }
            Ok(ids)
        }
    }

    fn filetime_to_system_time(ft: FILETIME) -> Option<SystemTime> {
        // FILETIME counts 100ns ticks from 1601-01-01.
        const EPOCH_OFFSET_SECS: u64 = 11_644_473_600;
        let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        let secs = ticks / 10_000_000;
        let nanos = (ticks % 10_000_000) * 100;
        secs.checked_sub(EPOCH_OFFSET_SECS)
            .map(|s| UNIX_EPOCH + Duration::new(s, nanos as u32))
    }

    unsafe fn read_modified(values: &IPortableDeviceValues) -> Option<SystemTime> {
        // SAFETY: the PROPVARIANT is owned by us and dropped at scope end.
        unsafe {
            let variant = values.GetValue(&OBJECT_DATE_MODIFIED).ok()?;
            let ft = PropVariantToFileTime(&variant, PSTF_UTC).ok()?;
            filetime_to_system_time(ft)
        }
    }

    struct ObjectInfo {
        name: String,
        is_dir: bool,
        size: u64,
        modified: Option<SystemTime>,
        hidden: bool,
    }

    unsafe fn describe(
        open: &Opened,
        keys: &IPortableDeviceKeyCollection,
        object: &str,
    ) -> Option<ObjectInfo> {
        // SAFETY: object ID came from the enumerator on this thread.
        unsafe {
            let values = open
                .properties
                .GetValues(&HSTRING::from(object), keys)
                .ok()?;

            let content_type = values.GetGuidValue(&OBJECT_CONTENT_TYPE).ok();
            let is_dir = matches!(
                content_type,
                Some(t) if t == CONTENT_FOLDER || t == CONTENT_FUNCTIONAL
            );

            // The original file name carries the extension; the display name is
            // the only thing storages have.
            let name = values
                .GetStringValue(&OBJECT_ORIGINAL_FILE_NAME)
                .ok()
                .map(|raw| take_string(raw))
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    values
                        .GetStringValue(&OBJECT_NAME)
                        .ok()
                        .map(|raw| take_string(raw))
                })
                .filter(|s| !s.is_empty())?;

            Some(ObjectInfo {
                name,
                is_dir,
                size: values
                    .GetUnsignedLargeIntegerValue(&OBJECT_SIZE)
                    .unwrap_or(0),
                modified: read_modified(&values),
                hidden: values
                    .GetBoolValue(&OBJECT_IS_HIDDEN)
                    .map(|b| b.as_bool())
                    .unwrap_or(false),
            })
        }
    }

    unsafe fn list_children(
        cache: &mut HashMap<String, Opened>,
        path: &Path,
    ) -> Result<Vec<Entry>> {
        let (key, object) = parse(path).ok_or_else(|| anyhow!("Not a portable device path."))?;
        // SAFETY: all handles stay on the worker thread.
        unsafe {
            let open = handle(cache, &key)?;
            let keys = property_keys(&[
                OBJECT_NAME,
                OBJECT_ORIGINAL_FILE_NAME,
                OBJECT_CONTENT_TYPE,
                OBJECT_SIZE,
                OBJECT_DATE_MODIFIED,
                OBJECT_IS_HIDDEN,
            ])
            .map_err(|e| anyhow!("Could not prepare the property list: {e}"))?;

            let ids =
                child_ids(open, &object).map_err(|e| anyhow!("Could not read that folder: {e}"))?;

            let mut entries = Vec::with_capacity(ids.len());
            for id in ids {
                let Some(info) = describe(open, &keys, &id) else {
                    continue;
                };
                if info.hidden {
                    continue;
                }
                entries.push(Entry {
                    path: path.join(&id),
                    name: info.name,
                    kind: if info.is_dir {
                        EntryKind::Directory
                    } else {
                        EntryKind::File
                    },
                    size: info.size,
                    modified: info.modified,
                    hidden: false,
                });
            }
            Ok(entries)
        }
    }

    unsafe fn storage_totals(open: &Opened) -> (u64, u64) {
        // SAFETY: worker-thread handles only.
        unsafe {
            let Ok(keys) = property_keys(&[STORAGE_CAPACITY, STORAGE_FREE]) else {
                return (0, 0);
            };
            let Ok(storages) = child_ids(open, DEVICE_OBJECT) else {
                return (0, 0);
            };
            let (mut free, mut total) = (0u64, 0u64);
            for storage in storages {
                let Ok(values) = open.properties.GetValues(&HSTRING::from(storage), &keys) else {
                    continue;
                };
                total += values
                    .GetUnsignedLargeIntegerValue(&STORAGE_CAPACITY)
                    .unwrap_or(0);
                free += values
                    .GetUnsignedLargeIntegerValue(&STORAGE_FREE)
                    .unwrap_or(0);
            }
            (free, total)
        }
    }

    unsafe fn friendly_name(manager: &IPortableDeviceManager, pnp_id: &HSTRING) -> String {
        // SAFETY: two-call pattern, buffer sized by the first call.
        unsafe {
            let mut len = 0u32;
            let _ = manager.GetDeviceFriendlyName(pnp_id, PWSTR::null(), &mut len);
            if len == 0 {
                return String::new();
            }
            let mut buffer = vec![0u16; len as usize];
            if manager
                .GetDeviceFriendlyName(pnp_id, PWSTR(buffer.as_mut_ptr()), &mut len)
                .is_err()
            {
                return String::new();
            }
            let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            String::from_utf16_lossy(&buffer[..end])
        }
    }

    unsafe fn enumerate(cache: &mut HashMap<String, Opened>) -> Vec<Device> {
        // SAFETY: manager is created and released on the worker thread.
        unsafe {
            let Ok(manager) = CoCreateInstance::<_, IPortableDeviceManager>(
                &CLSID_MANAGER,
                None,
                CLSCTX_INPROC_SERVER,
            ) else {
                return Vec::new();
            };
            // The manager snapshots the device list when created, so ask for a
            // fresh one on every poll or hot-plugs are invisible.
            let _ = manager.RefreshDeviceList();

            let mut count = 0u32;
            if manager
                .GetDevices(std::ptr::null_mut(), &mut count)
                .is_err()
            {
                return Vec::new();
            }
            if count == 0 {
                cache.clear();
                if let Ok(mut registry) = REGISTRY.lock() {
                    registry.clear();
                }
                return Vec::new();
            }

            let mut raw_ids = vec![PWSTR::null(); count as usize];
            if manager
                .GetDevices(raw_ids.as_mut_ptr(), &mut count)
                .is_err()
            {
                return Vec::new();
            }

            let pnp_ids: Vec<String> = raw_ids
                .into_iter()
                .take(count as usize)
                .map(|raw| take_string(raw))
                .filter(|id| !id.is_empty())
                .collect();

            if let Ok(mut registry) = REGISTRY.lock() {
                registry.clear();
                for id in &pnp_ids {
                    registry.insert(key_for(id), id.clone());
                }
            }
            // Drop handles for anything unplugged so the next connect reopens.
            cache.retain(|key, _| pnp_ids.iter().any(|id| key_for(id) == *key));

            let mut devices = Vec::new();
            for pnp_id in pnp_ids {
                let key = key_for(&pnp_id);
                let wide = HSTRING::from(pnp_id.as_str());
                let name = friendly_name(&manager, &wide);
                let name = if name.is_empty() {
                    "Portable Device".to_string()
                } else {
                    name
                };
                let (free, total) = match handle(cache, &key) {
                    Ok(open) => storage_totals(open),
                    Err(_) => (0, 0),
                };
                devices.push(Device {
                    key,
                    name,
                    free,
                    total,
                });
            }
            devices
        }
    }

    unsafe fn copy_to_temp(cache: &mut HashMap<String, Opened>, path: &Path) -> Result<PathBuf> {
        let (key, object) = parse(path).ok_or_else(|| anyhow!("Not a portable device path."))?;
        // SAFETY: worker-thread handles; the stream is read to completion here.
        unsafe {
            let open = handle(cache, &key)?;
            let keys = property_keys(&[
                OBJECT_NAME,
                OBJECT_ORIGINAL_FILE_NAME,
                OBJECT_CONTENT_TYPE,
                OBJECT_SIZE,
                OBJECT_DATE_MODIFIED,
                OBJECT_IS_HIDDEN,
            ])
            .map_err(|e| anyhow!("Could not prepare the property list: {e}"))?;
            let Some(info) = describe(open, &keys, &object) else {
                bail!("Could not read that item from the device.");
            };
            if info.is_dir {
                bail!("That is a folder on the device.");
            }

            let resources = open
                .content
                .Transfer()
                .map_err(|e| anyhow!("This device does not allow transfers: {e}"))?;
            let mut chunk = 0u32;
            let mut stream: Option<IStream> = None;
            resources
                .GetStream(
                    &HSTRING::from(object.as_str()),
                    &RESOURCE_DEFAULT,
                    STGM_READ.0,
                    &mut chunk,
                    &mut stream,
                )
                .map_err(|e| anyhow!("Could not read that file from the device: {e}"))?;
            let stream = stream.ok_or_else(|| anyhow!("The device returned no data."))?;

            let dir = std::env::temp_dir().join("ply-portable");
            std::fs::create_dir_all(&dir)?;
            let target = dir.join(&info.name);

            let mut buffer = vec![0u8; chunk.clamp(16 * 1024, 1024 * 1024) as usize];
            let mut out = Vec::new();
            loop {
                let mut read = 0u32;
                stream
                    .Read(
                        buffer.as_mut_ptr() as *mut _,
                        buffer.len() as u32,
                        Some(&mut read),
                    )
                    .ok()
                    .map_err(|e| anyhow!("Transfer failed: {e}"))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buffer[..read as usize]);
            }
            std::fs::write(&target, &out)?;
            Ok(target)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_and_path_safe() {
        let id = r"\\?\usb#vid_22d9&pid_2765#abc#{6ac27878-a6fa-4155-ba85-f98f491d4f33}";
        let key = key_for(id);
        assert_eq!(key, key_for(id));
        assert!(!key.contains('\\'));
    }

    #[cfg(windows)]
    #[test]
    fn round_trips_device_root() {
        let root = root_of("deadbeefdeadbeef");
        assert!(is_mtp(&root));
        // The root addresses WPD's synthetic device object.
        assert_eq!(
            parse(&root),
            Some(("deadbeefdeadbeef".to_string(), "DEVICE".to_string()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn parses_nested_objects() {
        let path = root_of("abc").join("s10001").join("o42");
        assert_eq!(parse(&path), Some(("abc".to_string(), "o42".to_string())));
    }

    #[cfg(windows)]
    #[test]
    fn ancestors_walk_back_to_the_device() {
        let path = root_of("abc").join("s10001").join("o42");
        let parent = path.parent().expect("nested object has a parent");
        assert_eq!(
            parse(parent),
            Some(("abc".to_string(), "s10001".to_string()))
        );
        let root = parent.parent().expect("storage sits under the device");
        assert_eq!(parse(root), Some(("abc".to_string(), "DEVICE".to_string())));
    }

    #[test]
    #[ignore = "requires a connected portable device"]
    fn probe_connected_devices() {
        let found = devices();
        println!("devices: {found:#?}");
        for device in &found {
            let root = device.root();
            match list(&root) {
                Ok(entries) => {
                    for entry in &entries {
                        println!("  {} [{:?}] {}", entry.name, entry.kind, entry.size);
                    }
                    if let Some(storage) = entries.first() {
                        println!("  -- inside {} --", storage.name);
                        match list(&storage.path) {
                            Ok(inner) => {
                                for entry in inner.iter().take(15) {
                                    println!("    {} [{:?}]", entry.name, entry.kind);
                                }
                            }
                            Err(err) => println!("    failed: {err}"),
                        }
                    }
                }
                Err(err) => println!("  failed: {err}"),
            }
        }
    }

    #[test]
    fn plain_paths_are_not_mtp() {
        assert!(!is_mtp(Path::new(r"C:\Users")));
        assert!(!is_mtp(Path::new("/home")));
    }
}
