//! Original process-tap adapter. No native capture is opened during discovery.
//! The source is one exact current set of audio processes in a saved outer app
//! bundle. A change requires Stop/Start; this is not a seamless relaunch bridge.
use super::{
    application::{ApplicationInfo, ProcessInstance},
    backend,
    capture::{checked_owner_bytes, CaptureState, OWNER_STACK_BYTES},
    native_capture::{self, Dependency, NativeCapture, PreparationError, Prepared},
    AudioError,
};
use objc2::{
    msg_send,
    rc::{autoreleasepool, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_core_audio::*;
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSNumber, NSString};
use relink_core::{ApplicationSelector, Uuid};
use std::{
    ffi::{c_void, CStr},
    mem::{size_of, MaybeUninit},
    path::{Path, PathBuf},
    ptr::NonNull,
};

const MAX_PATH: usize = 4096;
// Byte budgets, not instance-count policy. Every member is admitted or the
// complete selection fails; no prefix of a process set is ever captured.
const MEMBERSHIP_BYTES: usize = 1024 * 1024;
const ANCESTRY_BYTES: usize = 1024 * 1024;
// Includes the original owner stack, bounded property buffers, listener/context
// storage and stereo stream metadata. Actual native selection is checked below.
const NATIVE_OWNER_BOUND: usize = OWNER_STACK_BYTES + 1024 * 1024;
const MEMBERSHIP_ERROR: &str =
    "application process membership or identity changed; stop audio and start again";

type CreateTap = unsafe extern "C-unwind" fn(*const AnyObject, *mut u32) -> i32;
type DestroyTap = unsafe extern "C-unwind" fn(u32) -> i32;
#[derive(Clone, Copy)]
struct TapApi {
    create: CreateTap,
    destroy: DestroyTap,
}
impl TapApi {
    fn load() -> Result<Self, String> {
        // Do not directly import the 14.2 entrypoints: the application itself
        // still launches on macOS 13. Class presence alone is insufficient.
        let create = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"AudioHardwareCreateProcessTap".as_ptr(),
            )
        };
        let destroy = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"AudioHardwareDestroyProcessTap".as_ptr(),
            )
        };
        if create.is_null() || destroy.is_null() || AnyClass::get(c"CATapDescription").is_none() {
            return Err("application audio capture requires macOS 14.2 or later".into());
        }
        Ok(Self {
            // Both symbols are resolved from CoreAudio already linked for the
            // existing device backend, with signatures from AudioHardwareTapping.h.
            create: unsafe { std::mem::transmute::<*mut c_void, CreateTap>(create) },
            destroy: unsafe { std::mem::transmute::<*mut c_void, DestroyTap>(destroy) },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Member {
    object: u32,
    pid: u32,
    started: u64,
    executable: PathBuf,
}
#[derive(Clone)]
pub(crate) struct Selection {
    key: String,
    name: String,
    bundle_id: String,
    bundle: PathBuf,
    members: Vec<Member>,
    scope: Vec<ProcessInstance>,
    rate: u32,
}
impl Selection {
    pub(crate) fn key(&self) -> String {
        self.key.clone()
    }
    pub(crate) fn name(&self) -> String {
        self.name.clone()
    }
    pub(crate) fn configuration(&self) -> cpal::SupportedStreamConfig {
        cpal::SupportedStreamConfig::new(
            2,
            self.rate,
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        )
    }
    pub(crate) fn description(&self) -> String {
        format!("private stereo process tap; {} current bundle audio processes; restart on membership change", self.members.len())
    }
    pub(crate) fn scope(&self) -> &[ProcessInstance] {
        &self.scope
    }
    pub(crate) fn owner_bytes(&self) -> Result<usize, AudioError> {
        let stored = checked_owner_bytes([
            Some(size_of::<Self>()),
            Some(self.key.capacity()),
            Some(self.name.capacity()),
            Some(self.bundle_id.capacity()),
            Some(self.bundle.capacity()),
            self.members.capacity().checked_mul(size_of::<Member>()),
            self.members.iter().try_fold(0_usize, |sum, member| {
                sum.checked_add(member.executable.capacity())
            }),
            self.scope
                .capacity()
                .checked_mul(size_of::<ProcessInstance>()),
        ])?;
        checked_owner_bytes([
            Some(NATIVE_OWNER_BOUND),
            stored.checked_mul(3),
            Some(size_of::<Resources>()),
            // A complete bounded reinspection can coexist with the saved set.
            Some(MEMBERSHIP_BYTES),
            Some(ANCESTRY_BYTES),
            Some(2 * 64 * 1024 + 4 * MAX_PATH),
        ])
    }
    fn check(&self) -> Result<(), String> {
        let (bundle_id, _) = bundle_info(&self.bundle)?;
        if bundle_id != self.bundle_id
            || self.bundle.canonicalize().map_err(|e| e.to_string())? != self.bundle
        {
            return Err(MEMBERSHIP_ERROR.into());
        }
        if resolve_members(&self.bundle)? != self.members {
            return Err(MEMBERSHIP_ERROR.into());
        }
        Ok(())
    }
}

pub(crate) fn enumerate() -> Result<Vec<ApplicationInfo>, AudioError> {
    TapApi::load().map_err(|e| backend("application discovery", None, e))?;
    let mut applications: Vec<ApplicationInfo> = Vec::new();
    let mut application_bytes = 0;
    for object in process_objects().map_err(|e| backend("application discovery", None, e))? {
        let Ok(pid) = process_pid(object) else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let Ok(executable) = executable(pid) else {
            continue;
        };
        let Some(bundle) = outer_bundle(&executable) else {
            continue;
        };
        let Ok((bundle_id, name)) = bundle_info(&bundle) else {
            continue;
        };
        let Some(path) = bundle.to_str() else {
            continue;
        };
        let selector = ApplicationSelector::MacBundle {
            bundle_id,
            path: path.into(),
        };
        let Ok(id) = selector.encode() else { continue };
        if !applications.iter().any(|application| application.id == id) {
            application_bytes = admit_bytes(
                application_bytes,
                size_of::<ApplicationInfo>() + id.capacity() + name.capacity(),
                MEMBERSHIP_BYTES,
            )
            .map_err(|e| backend("application discovery", None, e))?;
            applications.try_reserve_exact(1).map_err(|_| {
                backend(
                    "application discovery",
                    None,
                    "application list allocation failed",
                )
            })?;
            applications.push(ApplicationInfo { id, name });
        }
    }
    applications.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(applications)
}
pub(crate) fn select(id: &str, rate: u32) -> Result<Vec<Selection>, AudioError> {
    let operation = || -> Result<Selection, String> {
        TapApi::load()?;
        if id.len() > 12 * MAX_PATH {
            return Err("saved application identity exceeds the native memory budget".into());
        }
        let selector = ApplicationSelector::decode(id).map_err(|e| e.to_string())?;
        let key = selector.encode().map_err(|e| e.to_string())?;
        let ApplicationSelector::MacBundle { bundle_id, path } = selector else {
            return Err("the saved application belongs to a different platform".into());
        };
        let bundle = Path::new(&path)
            .canonicalize()
            .map_err(|e| format!("saved application path is unavailable: {e}"))?;
        if bundle != Path::new(&path) || outer_bundle(&bundle).as_ref() != Some(&bundle) {
            return Err(
                "saved application path changed or is not an outer app bundle; select it again"
                    .into(),
            );
        }
        let (actual_id, name) = bundle_info(&bundle)?;
        if actual_id != bundle_id {
            return Err("saved application bundle identity changed; select it again".into());
        }
        let members = resolve_members(&bundle)?;
        if members.is_empty() {
            return Err("this application has no current CoreAudio process objects; launch it and start its audio before capture".into());
        }
        let scope = members
            .iter()
            .map(|m| ProcessInstance {
                pid: m.pid,
                started: m.started,
            })
            .collect();
        let selection = Selection {
            key,
            name,
            bundle_id,
            bundle,
            members,
            scope,
            rate,
        };
        selection.check()?;
        Ok(selection)
    };
    operation()
        .map(|selection| vec![selection])
        .map_err(|e| backend("application selection", Some(id.into()), e))
}

pub(crate) struct ApplicationCapture(NativeCapture);
impl ApplicationCapture {
    pub(crate) fn build(selection: &Selection, state: CaptureState) -> Result<Self, AudioError> {
        let plan = selection.clone();
        NativeCapture::build_prepared(&selection.key, state, move |authorized| {
            prepare(plan, authorized)
        })
        .map(Self)
    }
    pub(crate) fn play(&self) -> Result<(), AudioError> {
        self.0.play()
    }
}

fn process_objects() -> Result<Vec<u32>, String> {
    native_capture::objects(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyProcessObjectList,
        kAudioObjectPropertyScopeGlobal,
    )
}
fn process_pid(object: u32) -> Result<u32, String> {
    let pid: i32 = native_capture::property(
        object,
        native_capture::address(kAudioProcessPropertyPID, kAudioObjectPropertyScopeGlobal),
    )?;
    u32::try_from(pid)
        .ok()
        .filter(|pid| *pid != 0)
        .ok_or_else(|| "CoreAudio returned an invalid process ID".into())
}
fn process_info(pid: u32) -> Result<libc::proc_bsdinfo, String> {
    let pid = i32::try_from(pid).map_err(|_| "process ID is outside native range")?;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    if bytes as usize != size_of::<libc::proc_bsdinfo>() {
        return Err(format!("cannot verify process {pid} lifetime"));
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid as u32 || info.pbi_start_tvusec >= 1_000_000 {
        return Err("invalid process lifetime record".into());
    }
    Ok(info)
}
fn birth(info: &libc::proc_bsdinfo) -> Result<u64, String> {
    info.pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|t| t.checked_add(info.pbi_start_tvusec))
        .ok_or_else(|| "process lifetime overflow".into())
}
fn executable(pid: u32) -> Result<PathBuf, String> {
    let pid = i32::try_from(pid).map_err(|_| "process ID is outside native range")?;
    let mut bytes = [0_u8; MAX_PATH];
    let length = unsafe { libc::proc_pidpath(pid, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
    if length <= 0 || length as usize >= bytes.len() {
        return Err(format!("cannot verify process {pid} executable"));
    }
    let path = CStr::from_bytes_until_nul(&bytes).map_err(|_| "unterminated process path")?;
    let path = path.to_str().map_err(|_| "process path is not UTF-8")?;
    Path::new(path)
        .canonicalize()
        .map_err(|e| format!("cannot resolve process executable: {e}"))
}
fn outer_bundle(executable: &Path) -> Option<PathBuf> {
    // First .app from the root deliberately includes nested helper bundles in
    // their containing application, not as unrelated same-name applications.
    let mut path = PathBuf::new();
    for component in executable.components() {
        path.push(component);
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        {
            return Some(path);
        }
    }
    None
}
fn bundle_info(path: &Path) -> Result<(String, String), String> {
    // Discovery and monitoring run on worker threads without an AppKit event
    // loop. Bound Foundation's temporary autoreleased objects on every call.
    autoreleasepool(|_| bundle_info_inner(path))
}
fn bundle_info_inner(path: &Path) -> Result<(String, String), String> {
    let path_text = path
        .to_str()
        .ok_or("application bundle path is not UTF-8")?;
    if path_text.len() >= MAX_PATH {
        return Err("application bundle path exceeds bounded storage".into());
    }
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(path_text))
        .ok_or("application bundle is unavailable")?;
    let id = bundle
        .bundleIdentifier()
        .ok_or("application has no bundle identifier")?
        .to_string();
    if id.len() > MAX_PATH {
        return Err("application bundle identifier exceeds bounded storage".into());
    }
    // The verified bundle path is also a useful disambiguator for two copies
    // sharing an identifier. Do not trust an arbitrary Info.plist value as a
    // typed NSString or coerce non-string metadata through Objective-C.
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("application bundle has no name")?
        .to_owned();
    Ok((id, name))
}

fn resolve_members(bundle: &Path) -> Result<Vec<Member>, String> {
    let mut members = Vec::new();
    let mut member_bytes = 0_usize;
    let objects = process_objects()?;
    for object in objects {
        // A changing HAL list or an unresolvable client makes this snapshot
        // invalid; it might be another helper, so do not silently omit it.
        let pid = process_pid(object)?;
        let path = executable(pid)?;
        if outer_bundle(&path).as_deref() == Some(bundle) {
            let executable = path;
            let before = process_info(pid)?;
            if pid == std::process::id() {
                return Err(
                    "capturing ReLink itself would feed its output back into the mix".into(),
                );
            }
            let started = birth(&before)?;
            let after = process_info(pid)?;
            if birth(&after)? != started
                || executable != self::executable(pid)?
                || process_pid(object)? != pid
            {
                return Err(MEMBERSHIP_ERROR.into());
            }
            member_bytes = admit_bytes(
                member_bytes,
                size_of::<Member>() + size_of::<ProcessInstance>() + executable.capacity(),
                MEMBERSHIP_BYTES,
            )?;
            members
                .try_reserve_exact(1)
                .map_err(|_| "application membership allocation failed")?;
            members.push(Member {
                object,
                pid,
                started,
                executable,
            });
        } else if ancestor_in_bundle(pid, bundle)? {
            // A known external descendant would make our included set partial.
            return Err("application has an audio helper outside its saved bundle; its complete process capture is not supported".into());
        }
    }
    members.sort_by_key(|member| (member.pid, member.started, member.object));
    if members.windows(2).any(|pair| pair[0].pid == pair[1].pid) {
        return Err("duplicate CoreAudio process identity".into());
    }
    Ok(members)
}
fn admit_bytes(used: usize, additional: usize, budget: usize) -> Result<usize, String> {
    used.checked_add(additional)
        .filter(|total| *total <= budget)
        .ok_or_else(|| "complete application process verification exceeds its memory budget".into())
}
fn ancestor_in_bundle(mut pid: u32, bundle: &Path) -> Result<bool, String> {
    let mut visited = Vec::new();
    let mut previous_birth = u64::MAX;
    let mut bytes = 0;
    loop {
        let info = process_info(pid)?;
        let instance = ProcessInstance {
            pid,
            started: birth(&info)?,
        };
        if instance.started > previous_birth || visited.contains(&instance) {
            return Err("process ancestry changed during application discovery".into());
        }
        previous_birth = instance.started;
        bytes = admit_bytes(bytes, size_of::<ProcessInstance>(), ANCESTRY_BYTES)?;
        visited
            .try_reserve_exact(1)
            .map_err(|_| "process ancestry allocation failed")?;
        visited.push(instance);
        pid = info.pbi_ppid;
        if pid <= 1 {
            return Ok(false);
        }
        if outer_bundle(&executable(pid)?).as_deref() == Some(bundle) {
            return Ok(true);
        }
    }
}

struct Resources {
    api: TapApi,
    tap: u32,
    aggregate: u32,
    selection: Selection,
}
impl Dependency for Resources {
    fn check(&self) -> Result<(), String> {
        self.selection.check()
    }
    fn close(&mut self) -> Result<(), i32> {
        close_resources(
            &mut self.aggregate,
            &mut self.tap,
            |id| unsafe { AudioHardwareDestroyAggregateDevice(id) },
            |id| unsafe { (self.api.destroy)(id) },
        )
    }
}
fn close_resources(
    aggregate: &mut u32,
    tap: &mut u32,
    mut destroy_aggregate: impl FnMut(u32) -> i32,
    mut destroy_tap: impl FnMut(u32) -> i32,
) -> Result<(), i32> {
    if *aggregate != 0 {
        let status = destroy_aggregate(*aggregate);
        if status != 0 {
            return Err(status);
        }
        *aggregate = 0;
    }
    if *tap != 0 {
        let status = destroy_tap(*tap);
        if status != 0 {
            return Err(status);
        }
        *tap = 0;
    }
    Ok(())
}
fn native_status(status: i32, operation: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "{operation}: CoreAudio OSStatus {status}; system audio permission may be required"
        ))
    }
}
fn prepare(
    selection: Selection,
    authorized: &dyn Fn() -> bool,
) -> Result<Prepared, PreparationError> {
    let api = TapApi::load().map_err(|message| PreparationError {
        message,
        retained: false,
    })?;
    let mut resources = Box::new(Resources {
        api,
        tap: 0,
        aggregate: 0,
        selection,
    });
    match autoreleasepool(|_| prepare_resources(&mut resources, authorized)) {
        Ok(selection) => Ok(Prepared {
            selection,
            dependency: Some(resources),
        }),
        Err(mut message) => {
            let retained = match resources.close() {
                Ok(()) => false,
                Err(status) => {
                    message.push_str(&format!("; partial setup cleanup failed with OSStatus {status}; source is quarantined until restart"));
                    std::mem::forget(resources);
                    true
                }
            };
            Err(PreparationError { message, retained })
        }
    }
}
fn prepare_resources(
    resources: &mut Resources,
    authorized: &dyn Fn() -> bool,
) -> Result<native_capture::Selection, String> {
    resources.selection.check()?;
    let class =
        AnyClass::get(c"CATapDescription").ok_or("CoreAudio process taps are unavailable")?;
    let processes = NSArray::from_retained_slice(
        &resources
            .selection
            .members
            .iter()
            .map(|m| NSNumber::new_u32(m.object))
            .collect::<Vec<_>>(),
    );
    // All selectors are from the 14.2 tap API. In particular, no 26-only bundleID
    // restore property is sent to the class on earlier supported systems.
    let description: Retained<AnyObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![class, alloc];
        let initialized: *mut AnyObject =
            msg_send![allocated, initStereoMixdownOfProcesses: &*processes];
        Retained::from_raw(initialized).ok_or("CoreAudio tap description could not be allocated")?
    };
    unsafe {
        let _: () = msg_send![&*description, setPrivate: true];
        let _: () = msg_send![&*description, setMuteBehavior: 0_isize]; // CATapUnmuted
        let _: () =
            msg_send![&*description, setName: &*NSString::from_str("ReLink application capture")];
    }
    if !authorized() {
        return Err("application capture preparation was cancelled".into());
    }
    native_status(
        unsafe { (resources.api.create)(&*description, &mut resources.tap) },
        "create private process tap",
    )?;
    if resources.tap == 0 {
        return Err("CoreAudio returned no process tap".into());
    }
    let raw_uid: *mut CFString = native_capture::property(
        resources.tap,
        native_capture::address(kAudioTapPropertyUID, kAudioObjectPropertyScopeGlobal),
    )?;
    let tap_uid =
        unsafe { CFRetained::from_raw(NonNull::new(raw_uid).ok_or("process tap has no UID")?) }
            .to_string();
    let format: AudioStreamBasicDescription = native_capture::property(
        resources.tap,
        native_capture::address(kAudioTapPropertyFormat, kAudioObjectPropertyScopeGlobal),
    )?;
    if format.mSampleRate != f64::from(resources.selection.rate) || format.mChannelsPerFrame != 2 {
        return Err(format!("application tap exposes {} Hz/{} channels, but this mix requires {} Hz stereo; nominal sample-rate conversion is not implemented", format.mSampleRate, format.mChannelsPerFrame, resources.selection.rate));
    }
    let uid = format!("au.textual.relink.application.{}", Uuid::new_v4());
    let tap_uid = NSString::from_str(&tap_uid);
    let yes = NSNumber::new_bool(true);
    let no = NSNumber::new_bool(false);
    let sub: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&[&*NSString::from_str("uid")], &[&*tap_uid as &AnyObject]);
    let taps = NSArray::from_retained_slice(&[sub]);
    let uid_string = NSString::from_str(&uid);
    let name = NSString::from_str("ReLink private application input");
    let keys: Vec<_> = ["uid", "name", "private", "taps", "tapautostart"]
        .into_iter()
        .map(NSString::from_str)
        .collect();
    let dictionary: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
        &keys.iter().map(|k| &**k).collect::<Vec<_>>(),
        &[&*uid_string as &AnyObject, &*name, &*yes, &*taps, &*no],
    );
    // NSDictionary/CFDictionary are toll-free bridged; all contents match the
    // aggregate and subtap keys in the installed public CoreAudio SDK.
    let dictionary = unsafe {
        &*((&*dictionary as *const NSDictionary<NSString, AnyObject>).cast::<CFDictionary>())
    };
    if !authorized() {
        return Err("application capture preparation was cancelled".into());
    }
    native_status(
        unsafe {
            AudioHardwareCreateAggregateDevice(dictionary, NonNull::from(&mut resources.aggregate))
        },
        "create private tap aggregate",
    )?;
    if resources.aggregate == 0 {
        return Err("CoreAudio returned no tap aggregate".into());
    }
    let native = native_capture::inspect(&uid, resources.selection.rate)?;
    if native.configuration().channels() != 2
        || native.owner_bytes().map_err(|e| e.to_string())? > NATIVE_OWNER_BOUND
    {
        return Err("application aggregate exceeds the reserved stereo owner configuration".into());
    }
    resources.selection.check()?;
    if !authorized() {
        return Err("application capture preparation was cancelled".into());
    }
    Ok(native)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_admission_uses_bytes_without_a_fixed_instance_or_ancestry_count() {
        let mut bytes = 0;
        for _ in 0..1024 {
            bytes = admit_bytes(
                bytes,
                size_of::<Member>() + size_of::<ProcessInstance>() + 64,
                MEMBERSHIP_BYTES,
            )
            .unwrap();
        }
        assert!(bytes < MEMBERSHIP_BYTES);
        assert_eq!(
            admit_bytes(MEMBERSHIP_BYTES - 1, 1, MEMBERSHIP_BYTES),
            Ok(MEMBERSHIP_BYTES)
        );
        assert!(admit_bytes(MEMBERSHIP_BYTES, 1, MEMBERSHIP_BYTES).is_err());
        assert!(admit_bytes(usize::MAX, 1, usize::MAX).is_err());
        assert!(admit_bytes(0, size_of::<ProcessInstance>() * 1024, ANCESTRY_BYTES).is_ok());
    }
    #[test]
    fn helpers_belong_to_exact_outer_bundle_not_name_prefix_or_nested_bundle() {
        assert_eq!(
            outer_bundle(Path::new(
                "/Applications/Editor.app/Contents/Frameworks/Helper.app/Contents/MacOS/Helper"
            )),
            Some(PathBuf::from("/Applications/Editor.app"))
        );
        assert_ne!(
            outer_bundle(Path::new(
                "/Applications/Editor Copy.app/Contents/MacOS/Editor"
            )),
            Some(PathBuf::from("/Applications/Editor.app"))
        );
        assert!(outer_bundle(Path::new("/usr/local/bin/editor")).is_none());
    }
    #[test]
    fn dependency_cleanup_is_ordered_and_retains_tap_if_aggregate_cannot_close() {
        let (mut aggregate, mut tap) = (41, 42);
        let calls = std::cell::RefCell::new(Vec::new());
        assert_eq!(
            close_resources(
                &mut aggregate,
                &mut tap,
                |id| {
                    calls.borrow_mut().push(id);
                    -7
                },
                |id| {
                    calls.borrow_mut().push(id);
                    0
                }
            ),
            Err(-7)
        );
        assert_eq!(*calls.borrow(), [41]);
        assert_eq!((aggregate, tap), (41, 42));
        calls.borrow_mut().clear();
        assert_eq!(
            close_resources(
                &mut aggregate,
                &mut tap,
                |id| {
                    calls.borrow_mut().push(id);
                    0
                },
                |id| {
                    calls.borrow_mut().push(id);
                    -8
                }
            ),
            Err(-8)
        );
        assert_eq!(*calls.borrow(), [41, 42]);
        assert_eq!((aggregate, tap), (0, 42));
        calls.borrow_mut().clear();
        assert_eq!(
            close_resources(
                &mut aggregate,
                &mut tap,
                |id| {
                    calls.borrow_mut().push(id);
                    0
                },
                |id| {
                    calls.borrow_mut().push(id);
                    0
                }
            ),
            Ok(())
        );
        assert_eq!(*calls.borrow(), [42]);
        assert_eq!((aggregate, tap), (0, 0));
    }
}
