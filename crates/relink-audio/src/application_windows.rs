//! Resolve a persisted executable path into current, disjoint process trees.
//! PID/creation identities are held only for this run, never persisted as a selector.
pub(crate) use super::wasapi_application::ApplicationCapture;
use super::{
    application::{ApplicationInfo, ProcessInstance},
    backend, AudioError,
};
use relink_core::ApplicationSelector;
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
    path::Path,
    sync::Arc,
};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, ERROR_NO_MORE_FILES, FILETIME, HANDLE, WAIT_TIMEOUT},
        System::{Diagnostics::ToolHelp::*, Threading::*},
    },
};

const MAX_DISCOVERY_BYTES: usize = 16 * 1024 * 1024;
const ENTRY_BYTES: usize = size_of::<Entry>() + 260 * 4 + 128;
const SCOPE_ENTRY_BYTES: usize = 1024;
#[derive(Clone, Copy)]
struct DiscoveryBudget {
    snapshot: usize,
    scope: usize,
}
const MAX_IMAGE_UNITS: usize = 32768;

pub(crate) struct Handle(pub(crate) HANDLE);
// Process/snapshot handles are kernel handles usable from any thread. No COM
// interface or borrowed memory is carried by this wrapper.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Process {
    handle: Handle,
    instance: ProcessInstance,
}
impl Process {
    fn open(pid: u32) -> Result<Self, AudioError> {
        let handle = Handle(
            unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
            }
            .map_err(|e| {
                backend(
                    "application process identity",
                    Some(format!("PID {pid}")),
                    e,
                )
            })?,
        );
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
            .map_err(|e| {
                backend(
                    "application creation identity",
                    Some(format!("PID {pid}")),
                    e,
                )
            })?;
        let started = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        let result = Self {
            handle,
            instance: ProcessInstance { pid, started },
        };
        result.check_alive()?;
        Ok(result)
    }
    fn check_alive(&self) -> Result<(), AudioError> {
        if unsafe { WaitForSingleObject(self.handle.0, 0) } != WAIT_TIMEOUT {
            return Err(changed(
                "a selected process exited or could not be revalidated",
            ));
        }
        Ok(())
    }
    fn image(&self) -> Result<String, AudioError> {
        let mut data = vec![0_u16; MAX_IMAGE_UNITS];
        let mut length = data.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                self.handle.0,
                PROCESS_NAME_WIN32,
                PWSTR(data.as_mut_ptr()),
                &mut length,
            )
        }
        .map_err(|e| {
            backend(
                "application executable path",
                Some(format!("PID {}", self.instance.pid)),
                e,
            )
        })?;
        if length == 0 || length as usize >= data.len() {
            return Err(changed("the executable path exceeded bounded storage"));
        }
        let path = String::from_utf16(&data[..length as usize])
            .map_err(|_| changed("the executable path is not valid Unicode"))?;
        canonical(&path)
    }
}
fn canonical(path: &str) -> Result<String, AudioError> {
    let value = std::fs::canonicalize(path)
        .map_err(|e| backend("application canonical path", Some(path.into()), e))?;
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| changed("the canonical executable path is not valid Unicode"))
}
fn changed(detail: &str) -> AudioError {
    backend(
        "application capture identity",
        None,
        format!("{detail}; refresh the application list and Start again"),
    )
}
#[derive(Clone)]
struct Entry {
    pid: u32,
    parent: u32,
    image_name: String,
}
fn snapshot(byte_budget: usize) -> Result<BTreeMap<u32, Entry>, AudioError> {
    let handle = Handle(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|e| backend("application process discovery", None, e))?,
    );
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut result = BTreeMap::new();
    let mut next = unsafe { Process32FirstW(handle.0, &mut entry) };
    loop {
        match next {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
            Err(error) => return Err(backend("application process snapshot", None, error)),
        }
        if result
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_mul(ENTRY_BYTES))
            .is_none_or(|n| n > byte_budget)
        {
            return Err(changed(
                "process discovery exceeded its reserved byte budget; no partial list was accepted",
            ));
        }
        let end = entry
            .szExeFile
            .iter()
            .position(|&u| u == 0)
            .ok_or_else(|| changed("a process executable name is truncated"))?;
        let name = String::from_utf16(&entry.szExeFile[..end])
            .map_err(|_| changed("a process executable name is not valid Unicode"))?;
        if result
            .insert(
                entry.th32ProcessID,
                Entry {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    image_name: name,
                },
            )
            .is_some()
        {
            return Err(changed("the process snapshot repeated a PID"));
        }
        next = unsafe { Process32NextW(handle.0, &mut entry) };
    }
    Ok(result)
}

pub(crate) fn enumerate() -> Result<Vec<ApplicationInfo>, AudioError> {
    let entries = snapshot(MAX_DISCOVERY_BYTES)?;
    let own_path =
        std::env::current_exe().map_err(|e| backend("ReLink executable identity", None, e))?;
    let own_path = canonical(
        own_path
            .to_str()
            .ok_or_else(|| changed("ReLink's executable path is not valid Unicode"))?,
    )?;
    let mut paths = BTreeSet::new();
    let mut path_bytes = 0_usize;
    for entry in entries
        .values()
        .filter(|e| e.pid != 0 && e.pid != std::process::id())
    {
        // The picker lists queryable processes, not protected/system processes.
        // Selection is stricter: an inaccessible possible same-image instance
        // or descendant causes an explicit error instead of partial capture.
        if let Ok(process) = Process::open(entry.pid) {
            if let Ok(path) = process.image() {
                if path != own_path && !paths.contains(&path) {
                    path_bytes = path_bytes.checked_add(path.capacity() + 128).filter(|&n| n <= MAX_DISCOVERY_BYTES)
                        .ok_or_else(|| changed("application path discovery exceeded its byte budget; no partial list was accepted"))?;
                    paths.insert(path);
                }
            }
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let name = Path::new(&path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&path)
                .to_owned();
            let id = ApplicationSelector::WindowsExecutable { path }
                .encode()
                .map_err(|e| changed(&e.to_string()))?;
            Ok(ApplicationInfo { id, name })
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct Selection {
    path: Arc<str>,
    name: Arc<str>,
    root: ProcessInstance,
    scope: Vec<ProcessInstance>,
    matching: Arc<Vec<ProcessInstance>>,
    budget: DiscoveryBudget,
    // Holding these handles prevents identity records from being confused with
    // a fresh process using a recycled numeric PID. The API is rechecked too.
    held: Vec<Arc<Process>>,
    rate: u32,
}
impl Selection {
    pub(crate) fn key(&self) -> String {
        format!(
            "windows-process-tree:{}:{}",
            self.root.pid, self.root.started
        )
    }
    pub(crate) fn name(&self) -> String {
        self.name.to_string()
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
        "Windows process-tree loopback; stereo float; current instances only".into()
    }
    pub(crate) fn owner_bytes(&self) -> Result<usize, AudioError> {
        use super::capture::{checked_owner_bytes, OWNER_CHANNEL_BYTES, OWNER_STACK_BYTES};
        // Two stacks: native packet owner and non-realtime membership monitor.
        // Charge all owned identity copies plus two resolver scratch sets: the
        // monitor can validate concurrently with the owner's pre-Start check.
        // Native COM/audio-service private allocations are not inspectable here.
        checked_owner_bytes([
            OWNER_STACK_BYTES.checked_mul(2),
            Some(OWNER_CHANNEL_BYTES),
            Some(size_of::<Self>() + super::wasapi_application::fixed_owner_bytes()),
            self.path
                .len()
                .checked_add(self.name.len())
                .and_then(|n| n.checked_mul(4)),
            self.scope
                .capacity()
                .checked_add(self.matching.capacity())
                .and_then(|n| n.checked_mul(size_of::<ProcessInstance>() * 4)),
            self.held.capacity().checked_mul(
                (size_of::<Process>() + size_of::<Arc<Process>>() + 2 * size_of::<usize>()) * 4,
            ),
            self.budget.snapshot.checked_mul(2),
            self.budget.scope.checked_mul(2),
            Some(MAX_IMAGE_UNITS * 16 * 2),
        ])
    }
    pub(crate) fn scope(&self) -> &[ProcessInstance] {
        &self.scope
    }
    pub(crate) fn pid(&self) -> u32 {
        self.root.pid
    }
    pub(crate) fn validate(&self) -> Result<(), AudioError> {
        for process in &self.held {
            process.check_alive()?;
        }
        let selections = resolve(&self.path, self.rate, Some(self.budget))?;
        let current = selections
            .iter()
            .find(|s| s.root == self.root)
            .ok_or_else(|| changed("the selected process root changed"))?;
        if current.scope != self.scope || current.matching != self.matching {
            return Err(changed(
                "the application's current process membership changed",
            ));
        }
        Ok(())
    }
}

pub(crate) fn select(id: &str, rate: u32) -> Result<Vec<Selection>, AudioError> {
    let selector = ApplicationSelector::decode(id).map_err(|e| changed(&e.to_string()))?;
    let ApplicationSelector::WindowsExecutable { path } = selector else {
        return Err(AudioError::InvalidInputId(id.into()));
    };
    if !matches!(rate, 44100 | 48000 | 88200 | 96000 | 176400 | 192000) {
        return Err(AudioError::UnsupportedInputConfiguration {
            device: path,
            sample_rate: rate,
        });
    }
    let canonical_path = canonical(&path)?;
    // A moved/replaced alias is an explicit selector change, not a default.
    if canonical_path != path {
        return Err(changed(
            "the saved executable path no longer resolves to the same canonical path",
        ));
    }
    resolve(&path, rate, None)
}
fn resolve(
    path: &str,
    rate: u32,
    reserved: Option<DiscoveryBudget>,
) -> Result<Vec<Selection>, AudioError> {
    let entries = snapshot(reserved.map_or(MAX_DISCOVERY_BYTES, |b| b.snapshot))?;
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| changed("the selected path has no executable name"))?;
    let mut matching = BTreeSet::new();
    let mut held: BTreeMap<u32, Arc<Process>> = BTreeMap::new();
    for entry in entries
        .values()
        .filter(|e| e.image_name.eq_ignore_ascii_case(name))
    {
        let process = Arc::new(Process::open(entry.pid)?);
        if process.image()? == path {
            if matching
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_mul(SCOPE_ENTRY_BYTES))
                .is_none_or(|n| n > reserved.map_or(MAX_DISCOVERY_BYTES, |b| b.scope))
            {
                return Err(changed("selected instances exceeded their reserved byte budget; no partial selection was accepted"));
            }
            matching.insert(entry.pid);
            held.insert(entry.pid, process);
        }
    }
    if matching.is_empty() {
        return Err(changed(
            "no current process matches the selected executable",
        ));
    }
    // Expand every potentially included descendant before pruning roots. We
    // never silently omit inaccessible descendants or an over-capacity tree.
    let mut descendants = matching.clone();
    loop {
        let before = descendants.len();
        for entry in entries.values() {
            if descendants.contains(&entry.parent) && entry.pid != entry.parent {
                descendants.insert(entry.pid);
            }
        }
        if descendants
            .len()
            .checked_mul(SCOPE_ENTRY_BYTES)
            .is_none_or(|n| n > reserved.map_or(MAX_DISCOVERY_BYTES, |b| b.scope))
        {
            return Err(changed("the selected process trees exceed their reserved byte budget; no partial tree was accepted"));
        }
        if descendants.len() == before {
            break;
        }
    }
    for pid in &descendants {
        if !held.contains_key(pid) {
            held.insert(*pid, Arc::new(Process::open(*pid)?));
        }
    }
    let nodes: Vec<Node> = descendants
        .iter()
        .map(|pid| Node {
            pid: *pid,
            parent: entries[pid].parent,
            started: held[pid].instance.started,
        })
        .collect();
    let groups = partition(&nodes, &matching).map_err(changed)?;
    reject_self(&groups, std::process::id()).map_err(changed)?;
    let identities: Arc<Vec<_>> = Arc::new(matching.iter().map(|pid| held[pid].instance).collect());
    let budget = reserved.unwrap_or(DiscoveryBudget {
        snapshot: (entries.len() * ENTRY_BYTES * 2 + 65536).min(MAX_DISCOVERY_BYTES),
        scope: (descendants.len() * SCOPE_ENTRY_BYTES * 2 + 4096).min(MAX_DISCOVERY_BYTES),
    });
    let path: Arc<str> = path.into();
    let name: Arc<str> = name.into();
    let mut result = Vec::new();
    for (root, members) in groups {
        let scope: Vec<_> = members.iter().map(|pid| held[pid].instance).collect();
        let owners: Vec<_> = members.iter().map(|pid| held[pid].clone()).collect();
        for process in &owners {
            process.check_alive()?;
        }
        result.push(Selection {
            path: path.clone(),
            name: name.clone(),
            root: held[&root].instance,
            scope,
            matching: identities.clone(),
            budget,
            held: owners,
            rate,
        });
    }
    Ok(result)
}

// Pure topology rules used directly by resolution and offline controls.
#[derive(Clone, Copy)]
struct Node {
    pid: u32,
    parent: u32,
    started: u64,
}
fn partition(
    nodes: &[Node],
    matching: &BTreeSet<u32>,
) -> Result<Vec<(u32, Vec<u32>)>, &'static str> {
    let map: BTreeMap<_, _> = nodes.iter().map(|n| (n.pid, n)).collect();
    if map.len() != nodes.len() || matching.iter().any(|p| !map.contains_key(p)) {
        return Err("incomplete or repeated process topology");
    }
    let parent = |pid: u32| -> Option<u32> {
        let child = map.get(&pid)?;
        let ancestor = map.get(&child.parent)?;
        // A child cannot belong to a newer process that reused its parent PID.
        (ancestor.started <= child.started && ancestor.pid != child.pid).then_some(ancestor.pid)
    };
    let mut roots = Vec::new();
    for &pid in matching {
        let mut cursor = pid;
        let mut visited = BTreeSet::from([pid]);
        let mut covered = false;
        while let Some(next) = parent(cursor) {
            if !visited.insert(next) {
                return Err("cyclic process topology");
            }
            covered |= matching.contains(&next);
            cursor = next;
        }
        if !covered {
            roots.push(pid);
        }
    }
    let mut result = Vec::new();
    let mut assigned = BTreeSet::new();
    for root in roots {
        let mut members = BTreeSet::from([root]);
        loop {
            let before = members.len();
            for node in nodes {
                if parent(node.pid).is_some_and(|p| members.contains(&p)) {
                    members.insert(node.pid);
                }
            }
            if before == members.len() {
                break;
            }
        }
        for &pid in &members {
            if !assigned.insert(pid) {
                return Err("overlapping selected process trees");
            }
        }
        result.push((root, members.into_iter().collect()));
    }
    if !matching.is_subset(&assigned) {
        return Err("selected process instances were omitted");
    }
    Ok(result)
}

fn reject_self(groups: &[(u32, Vec<u32>)], current_pid: u32) -> Result<(), &'static str> {
    if groups.iter().any(|(_, scope)| scope.contains(&current_pid)) {
        Err("the selected process tree includes ReLink itself and would capture its own mixer output")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn self_capture_is_rejected_for_direct_and_ancestor_selections() {
        assert!(reject_self(&[(9, vec![9])], 9).is_err());
        assert!(reject_self(&[(1, vec![1, 2, 9])], 9).is_err());
        assert!(reject_self(&[(1, vec![1, 2]), (3, vec![3])], 9).is_ok());
    }
    #[test]
    fn roots_include_every_instance_and_prune_transitive_descendants() {
        let nodes = [
            Node {
                pid: 1,
                parent: 0,
                started: 1,
            },
            Node {
                pid: 2,
                parent: 1,
                started: 2,
            },
            Node {
                pid: 3,
                parent: 2,
                started: 3,
            },
            Node {
                pid: 4,
                parent: 0,
                started: 4,
            },
        ];
        assert_eq!(
            partition(&nodes, &BTreeSet::from([1, 3, 4])).unwrap(),
            vec![(1, vec![1, 2, 3]), (4, vec![4])]
        );
    }
    #[test]
    fn recycled_parent_pid_does_not_hide_an_older_matching_instance() {
        let nodes = [
            Node {
                pid: 1,
                parent: 0,
                started: 30,
            },
            Node {
                pid: 2,
                parent: 1,
                started: 20,
            },
        ];
        assert_eq!(
            partition(&nodes, &BTreeSet::from([1, 2])).unwrap(),
            vec![(1, vec![1]), (2, vec![2])]
        );
    }
    #[test]
    fn missing_or_cyclic_identity_fails_without_partial_capture() {
        let nodes = [
            Node {
                pid: 1,
                parent: 2,
                started: 1,
            },
            Node {
                pid: 2,
                parent: 1,
                started: 1,
            },
        ];
        assert!(partition(&nodes, &BTreeSet::from([1, 2])).is_err());
        assert!(partition(&nodes, &BTreeSet::from([3])).is_err());
    }
}
