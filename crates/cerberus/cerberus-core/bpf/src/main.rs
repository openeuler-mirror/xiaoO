#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use aya_ebpf::{
    helpers::{
        bpf_get_current_cgroup_id, bpf_get_current_pid_tgid, bpf_probe_read_user,
        bpf_probe_read_user_str_bytes,
    },
    macros::{map, tracepoint},
    maps::{HashMap, PerCpuArray, PerfEventArray},
    programs::TracePointContext,
};

const CLONE_THREAD: u64 = 0x00010000;

#[map(name = "EVENTS")]
static mut EVENTS: PerfEventArray<Event> = PerfEventArray::new(0);

#[map(name = "CONFIG")]
static mut CONFIG: HashMap<u32, Config> = HashMap::with_max_entries(1, 0);

#[map(name = "FORK_COUNT")]
static mut FORK_COUNT: HashMap<u64, u64> = HashMap::with_max_entries(1024, 0);

#[map(name = "OPENAT_ARGS")]
static mut OPENAT_ARGS: HashMap<u64, OpenatArgs> = HashMap::with_max_entries(1024, 0);

#[map(name = "CLONE_ARGS")]
static mut CLONE_ARGS: HashMap<u64, u64> = HashMap::with_max_entries(1024, 0);

#[map(name = "EVENT_BUF")]
static mut EVENT_BUF: PerCpuArray<Event> = PerCpuArray::with_max_entries(1, 0);

#[map(name = "SOCKET_ARGS")]
static mut SOCKET_ARGS: HashMap<u64, SocketArgs> = HashMap::with_max_entries(1024, 0);

#[map(name = "CONNECT_ARGS")]
static mut CONNECT_ARGS: HashMap<u64, ConnectArgs> = HashMap::with_max_entries(1024, 0);

#[map(name = "BIND_ARGS")]
static mut BIND_ARGS: HashMap<u64, BindArgs> = HashMap::with_max_entries(1024, 0);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Config {
    pub target_cgroup_id: u64,
    pub max_forks: u64,
    pub enabled: u32,
    pub _padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub event_type: u32,
    pub pid: u32,
    pub cgroup_id: u64,
    pub timestamp: u64,
    pub data: [u8; 256],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpenatArgs {
    pub flags: u32,
    pub mode: u32,
    pub pathname_ptr: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockaddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SocketArgs {
    pub domain: u32,
    pub socket_type: u32,
    pub protocol: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConnectArgs {
    pub sockaddr_ptr: u64, // 8 bytes at offset 0
    pub fd: u32,           // 4 bytes at offset 8
    pub addrlen: u32,      // 4 bytes at offset 12, total 16 bytes
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BindArgs {
    pub sockaddr_ptr: u64, // 8 bytes at offset 0
    pub fd: u32,           // 4 bytes at offset 8
    pub addrlen: u32,      // 4 bytes at offset 12, total 16 bytes
}

const EVENT_FILE_ACCESS: u32 = 1;
const EVENT_FORK: u32 = 2;
const EVENT_EXEC: u32 = 3;
const EVENT_NETWORK: u32 = 4;

const AF_INET: u16 = 2;
const SOCK_STREAM: u32 = 1;
const SOCK_DGRAM: u32 = 2;
const SOCK_TYPE_MASK: u32 = 0xF;

const NETWORK_DIRECTION_OFFSET: usize = 0;
const NETWORK_PROTOCOL_OFFSET: usize = 4;
const NETWORK_ADDRESS_OFFSET: usize = 8;
const NETWORK_PORT_OFFSET: usize = 12;

const NETWORK_DIRECTION_OUTBOUND: u32 = 0;
const NETWORK_DIRECTION_INBOUND: u32 = 1;

const NETWORK_PROTOCOL_TCP: u32 = 1;
const NETWORK_PROTOCOL_UDP: u32 = 2;

const CONFIG_KEY: u32 = 0;

#[inline(always)]
fn get_tgid() -> u32 {
    let pid_tgid = bpf_get_current_pid_tgid();
    (pid_tgid >> 32) as u32
}

#[inline(always)]
fn get_config() -> Option<&'static Config> {
    unsafe {
        let key = CONFIG_KEY;
        CONFIG.get(&key)
    }
}

#[inline(always)]
fn check_cgroup_filter() -> bool {
    unsafe {
        let config = match get_config() {
            Some(c) => c,
            None => return true,
        };

        if config.enabled == 0 || config.target_cgroup_id == 0 {
            return true;
        }

        let current_cgroup = bpf_get_current_cgroup_id();
        current_cgroup == config.target_cgroup_id
    }
}

fn emit_event(ctx: &TracePointContext, event: &Event) {
    unsafe {
        EVENTS.output(ctx, event, 0);
    }
}

fn tracepoint_read_arg<T: Copy>(ctx: &TracePointContext, offset: usize) -> Result<T, i64> {
    unsafe { ctx.read_at(offset) }
}

fn get_event_buf() -> Option<&'static mut Event> {
    unsafe { EVENT_BUF.get_ptr_mut(0).map(|p| &mut *p) }
}

#[inline(always)]
fn socket_key(pid: u32, fd: u32) -> u64 {
    ((pid as u64) << 32) | fd as u64
}

#[inline(always)]
fn protocol_from_socket_type(socket_type: u32) -> Option<u32> {
    match socket_type & SOCK_TYPE_MASK {
        SOCK_STREAM => Some(NETWORK_PROTOCOL_TCP),
        SOCK_DGRAM => Some(NETWORK_PROTOCOL_UDP),
        _ => None,
    }
}

fn read_sockaddr_in(sockaddr_ptr: u64, addrlen: u32) -> Option<SockaddrIn> {
    if sockaddr_ptr == 0 {
        return None;
    }

    if addrlen < core::mem::size_of::<SockaddrIn>() as u32 {
        return None;
    }

    let sockaddr = unsafe { bpf_probe_read_user(sockaddr_ptr as *const SockaddrIn).ok()? };
    if sockaddr.sin_family != AF_INET {
        return None;
    }

    Some(sockaddr)
}

fn write_network_data(data: &mut [u8; 256], direction: u32, protocol: u32, sockaddr: &SockaddrIn) {
    let port = u16::from_be(sockaddr.sin_port);
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);
        core::ptr::write(
            data.as_mut_ptr().add(NETWORK_DIRECTION_OFFSET) as *mut u32,
            direction,
        );
        core::ptr::write(
            data.as_mut_ptr().add(NETWORK_PROTOCOL_OFFSET) as *mut u32,
            protocol,
        );
        core::ptr::write(
            data.as_mut_ptr().add(NETWORK_ADDRESS_OFFSET) as *mut u32,
            sockaddr.sin_addr,
        );
        core::ptr::write(data.as_mut_ptr().add(NETWORK_PORT_OFFSET) as *mut u16, port);
    }
}

#[tracepoint(category = "syscalls", name = "sys_enter_openat")]
pub fn syscalls_sys_enter_openat(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pathname_ptr: u64 = match tracepoint_read_arg(&ctx, 24) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let flags: u32 = match tracepoint_read_arg(&ctx, 32) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let mode: u32 = match tracepoint_read_arg(&ctx, 40) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let args = OpenatArgs {
        flags,
        mode,
        pathname_ptr,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = OPENAT_ARGS.insert(&pid_tgid, &args, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_socket")]
pub fn syscalls_sys_enter_socket(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let domain: u32 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let socket_type: u32 = match tracepoint_read_arg(&ctx, 24) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let protocol: u32 = match tracepoint_read_arg(&ctx, 32) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let args = SocketArgs {
        domain,
        socket_type,
        protocol,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = SOCKET_ARGS.insert(&pid_tgid, &args, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_socket")]
pub fn syscalls_sys_exit_socket(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let retval: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let pid = get_tgid();
    let pid_tgid = bpf_get_current_pid_tgid();
    let args = unsafe {
        match SOCKET_ARGS.get(&pid_tgid) {
            Some(a) => *a,
            None => return 0,
        }
    };

    unsafe {
        let _ = SOCKET_ARGS.remove(&pid_tgid);
    }

    if retval < 0 || args.domain != AF_INET as u32 {
        return 0;
    }

    let fd = retval as u32;
    let key = socket_key(pid, fd);
    unsafe {
        let _ = SOCKET_ARGS.insert(&key, &args, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_connect")]
pub fn syscalls_sys_enter_connect(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let fd: u32 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let sockaddr_ptr: u64 = match tracepoint_read_arg(&ctx, 24) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let addrlen: u32 = match tracepoint_read_arg(&ctx, 32) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let args = ConnectArgs {
        fd,
        sockaddr_ptr,
        addrlen,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = CONNECT_ARGS.insert(&pid_tgid, &args, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_connect")]
pub fn syscalls_sys_exit_connect(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let _retval: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    let args = unsafe {
        match CONNECT_ARGS.get(&pid_tgid) {
            Some(a) => *a,
            None => return 0,
        }
    };

    unsafe {
        let _ = CONNECT_ARGS.remove(&pid_tgid);
    }

    let sockaddr = match read_sockaddr_in(args.sockaddr_ptr, args.addrlen) {
        Some(s) => s,
        None => return 0,
    };

    let socket_key = socket_key(pid, args.fd);
    let socket_args = unsafe { SOCKET_ARGS.get(&socket_key).copied() };
    let protocol = match socket_args.and_then(|a| protocol_from_socket_type(a.socket_type)) {
        Some(p) => p,
        None => return 0,
    };

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_NETWORK;
    event.pid = pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    write_network_data(
        &mut event.data,
        NETWORK_DIRECTION_OUTBOUND,
        protocol,
        &sockaddr,
    );

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_bind")]
pub fn syscalls_sys_enter_bind(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let fd: u32 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let sockaddr_ptr: u64 = match tracepoint_read_arg(&ctx, 24) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let addrlen: u32 = match tracepoint_read_arg(&ctx, 32) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let args = BindArgs {
        fd,
        sockaddr_ptr,
        addrlen,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = BIND_ARGS.insert(&pid_tgid, &args, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_bind")]
pub fn syscalls_sys_exit_bind(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let _retval: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    let args = unsafe {
        match BIND_ARGS.get(&pid_tgid) {
            Some(a) => *a,
            None => return 0,
        }
    };

    unsafe {
        let _ = BIND_ARGS.remove(&pid_tgid);
    }

    let sockaddr = match read_sockaddr_in(args.sockaddr_ptr, args.addrlen) {
        Some(s) => s,
        None => return 0,
    };

    let socket_key = socket_key(pid, args.fd);
    let socket_args = unsafe { SOCKET_ARGS.get(&socket_key).copied() };
    let protocol = match socket_args.and_then(|a| protocol_from_socket_type(a.socket_type)) {
        Some(p) => p,
        None => return 0,
    };

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_NETWORK;
    event.pid = pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    write_network_data(
        &mut event.data,
        NETWORK_DIRECTION_INBOUND,
        protocol,
        &sockaddr,
    );

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_openat")]
pub fn syscalls_sys_exit_openat(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let retval: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    let args = unsafe {
        match OPENAT_ARGS.get(&pid_tgid) {
            Some(a) => *a,
            None => return 0,
        }
    };

    unsafe {
        let _ = OPENAT_ARGS.remove(&pid_tgid);
    }

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_FILE_ACCESS;
    event.pid = pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);
    }

    let flags_offset = 0;
    let mode_offset = 4;
    let retval_offset = 8;
    let path_offset = 16;

    unsafe {
        core::ptr::write(data.as_mut_ptr().add(flags_offset) as *mut u32, args.flags);
        core::ptr::write(data.as_mut_ptr().add(mode_offset) as *mut u32, args.mode);
        core::ptr::write(data.as_mut_ptr().add(retval_offset) as *mut i64, retval);

        if args.pathname_ptr != 0 {
            let _ = bpf_probe_read_user_str_bytes(
                args.pathname_ptr as *const u8,
                &mut data[path_offset..],
            );
        }
    }

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_openat2")]
pub fn syscalls_sys_enter_openat2(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pathname_ptr: u64 = match tracepoint_read_arg(&ctx, 24) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let flags: u32 = match tracepoint_read_arg(&ctx, 32) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let args = OpenatArgs {
        flags,
        mode: 0,
        pathname_ptr,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = OPENAT_ARGS.insert(&pid_tgid, &args, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_openat2")]
pub fn syscalls_sys_exit_openat2(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let retval: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    let args = unsafe {
        match OPENAT_ARGS.get(&pid_tgid) {
            Some(a) => *a,
            None => return 0,
        }
    };

    unsafe {
        let _ = OPENAT_ARGS.remove(&pid_tgid);
    }

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_FILE_ACCESS;
    event.pid = pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);
        core::ptr::write(data.as_mut_ptr() as *mut u32, args.flags);
        core::ptr::write(data.as_mut_ptr().add(4) as *mut u32, args.mode);
        core::ptr::write(data.as_mut_ptr().add(8) as *mut i64, retval);

        if args.pathname_ptr != 0 {
            let _ = bpf_probe_read_user_str_bytes(args.pathname_ptr as *const u8, &mut data[16..]);
        }
    }

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_clone")]
pub fn syscalls_sys_enter_clone(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let clone_flags: u64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = CLONE_ARGS.insert(&pid_tgid, &clone_flags, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_clone")]
pub fn syscalls_sys_exit_clone(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let parent_pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let child_pid: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    if child_pid <= 0 {
        let pid_tgid = bpf_get_current_pid_tgid();
        unsafe {
            let _ = CLONE_ARGS.remove(&pid_tgid);
        }
        return 0;
    }

    let pid_tgid = bpf_get_current_pid_tgid();
    let clone_flags = unsafe { CLONE_ARGS.get(&pid_tgid).copied().unwrap_or(0) };

    unsafe {
        let _ = CLONE_ARGS.remove(&pid_tgid);
    }

    if (clone_flags & CLONE_THREAD) != 0 {
        return 0;
    }

    let fork_count = unsafe {
        let current = FORK_COUNT.get(&cgroup_id).copied().unwrap_or(0);
        let new_count = current + 1;
        let _ = FORK_COUNT.insert(&cgroup_id, &new_count, 0);
        new_count
    };

    let config = get_config();
    if config.map_or(0, |c| c.max_forks) > 0 && fork_count > config.unwrap().max_forks {
        let _ = unsafe { aya_ebpf::helpers::bpf_send_signal(9) };
    }

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_FORK;
    event.pid = parent_pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);
        core::ptr::write(data.as_mut_ptr() as *mut u32, parent_pid);
        core::ptr::write(data.as_mut_ptr().add(4) as *mut u32, child_pid as u32);
        core::ptr::write(data.as_mut_ptr().add(8) as *mut u64, fork_count);
        core::ptr::write(data.as_mut_ptr().add(16) as *mut u64, clone_flags);
    }

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_clone3")]
pub fn syscalls_sys_enter_clone3(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let args_ptr: u64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = CLONE_ARGS.insert(&pid_tgid, &args_ptr, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_clone3")]
pub fn syscalls_sys_exit_clone3(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let parent_pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let child_pid: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    if child_pid <= 0 {
        let pid_tgid = bpf_get_current_pid_tgid();
        unsafe {
            let _ = CLONE_ARGS.remove(&pid_tgid);
        }
        return 0;
    }

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = CLONE_ARGS.remove(&pid_tgid);
    }

    let fork_count = unsafe {
        let current = FORK_COUNT.get(&cgroup_id).copied().unwrap_or(0);
        let new_count = current + 1;
        let _ = FORK_COUNT.insert(&cgroup_id, &new_count, 0);
        new_count
    };

    let config = get_config();
    if config.map_or(0, |c| c.max_forks) > 0 && fork_count > config.unwrap().max_forks {
        let _ = unsafe { aya_ebpf::helpers::bpf_send_signal(9) };
    }

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_FORK;
    event.pid = parent_pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);
        core::ptr::write(data.as_mut_ptr() as *mut u32, parent_pid);
        core::ptr::write(data.as_mut_ptr().add(4) as *mut u32, child_pid as u32);
        core::ptr::write(data.as_mut_ptr().add(8) as *mut u64, fork_count);
    }

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_fork")]
pub fn syscalls_sys_enter_fork(_ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid_tgid = bpf_get_current_pid_tgid();
    let flags: u64 = 0;
    unsafe {
        let _ = CLONE_ARGS.insert(&pid_tgid, &flags, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_fork")]
pub fn syscalls_sys_exit_fork(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let parent_pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let child_pid: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    if child_pid <= 0 {
        let pid_tgid = bpf_get_current_pid_tgid();
        unsafe {
            let _ = CLONE_ARGS.remove(&pid_tgid);
        }
        return 0;
    }

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = CLONE_ARGS.remove(&pid_tgid);
    }

    let fork_count = unsafe {
        let current = FORK_COUNT.get(&cgroup_id).copied().unwrap_or(0);
        let new_count = current + 1;
        let _ = FORK_COUNT.insert(&cgroup_id, &new_count, 0);
        new_count
    };

    let config = get_config();
    if config.map_or(0, |c| c.max_forks) > 0 && fork_count > config.unwrap().max_forks {
        let _ = unsafe { aya_ebpf::helpers::bpf_send_signal(9) };
    }

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_FORK;
    event.pid = parent_pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);
        core::ptr::write(data.as_mut_ptr() as *mut u32, parent_pid);
        core::ptr::write(data.as_mut_ptr().add(4) as *mut u32, child_pid as u32);
        core::ptr::write(data.as_mut_ptr().add(8) as *mut u64, fork_count);
    }

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_vfork")]
pub fn syscalls_sys_enter_vfork(_ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid_tgid = bpf_get_current_pid_tgid();
    let flags: u64 = 0;
    unsafe {
        let _ = CLONE_ARGS.insert(&pid_tgid, &flags, 0);
    }

    0
}

#[tracepoint(category = "syscalls", name = "sys_exit_vfork")]
pub fn syscalls_sys_exit_vfork(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let parent_pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let child_pid: i64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    if child_pid <= 0 {
        let pid_tgid = bpf_get_current_pid_tgid();
        unsafe {
            let _ = CLONE_ARGS.remove(&pid_tgid);
        }
        return 0;
    }

    let pid_tgid = bpf_get_current_pid_tgid();
    unsafe {
        let _ = CLONE_ARGS.remove(&pid_tgid);
    }

    let fork_count = unsafe {
        let current = FORK_COUNT.get(&cgroup_id).copied().unwrap_or(0);
        let new_count = current + 1;
        let _ = FORK_COUNT.insert(&cgroup_id, &new_count, 0);
        new_count
    };

    let config = get_config();
    if config.map_or(0, |c| c.max_forks) > 0 && fork_count > config.unwrap().max_forks {
        let _ = unsafe { aya_ebpf::helpers::bpf_send_signal(9) };
    }

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_FORK;
    event.pid = parent_pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);
        core::ptr::write(data.as_mut_ptr() as *mut u32, parent_pid);
        core::ptr::write(data.as_mut_ptr().add(4) as *mut u32, child_pid as u32);
        core::ptr::write(data.as_mut_ptr().add(8) as *mut u64, fork_count);
    }

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_execve")]
pub fn syscalls_sys_enter_execve(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let filename_ptr: u64 = match tracepoint_read_arg(&ctx, 16) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_EXEC;
    event.pid = pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);

        if filename_ptr != 0 {
            let _ = bpf_probe_read_user_str_bytes(filename_ptr as *const u8, data);
        }
    }

    emit_event(&ctx, event);

    0
}

#[tracepoint(category = "syscalls", name = "sys_enter_execveat")]
pub fn syscalls_sys_enter_execveat(ctx: TracePointContext) -> i32 {
    if !check_cgroup_filter() {
        return 0;
    }

    let pid = get_tgid();
    let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
    let timestamp = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };

    let filename_ptr: u64 = match tracepoint_read_arg(&ctx, 24) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let event = match get_event_buf() {
        Some(e) => e,
        None => return 0,
    };

    event.event_type = EVENT_EXEC;
    event.pid = pid;
    event.cgroup_id = cgroup_id;
    event.timestamp = timestamp;

    let data = &mut event.data;
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, 256);

        if filename_ptr != 0 {
            let _ = bpf_probe_read_user_str_bytes(filename_ptr as *const u8, data);
        }
    }

    emit_event(&ctx, event);

    0
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
