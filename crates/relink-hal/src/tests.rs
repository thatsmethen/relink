use super::*;
use core::mem::{offset_of, size_of};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::Mutex,
};

thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
struct CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.with(|n| n.set(n.get() + 1));
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout)
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.with(|n| n.set(n.get() + 1));
        }
        System.realloc(p, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
// Only lifecycle tests mutate the process-wide plug-in registry.
static LIFECYCLE: Mutex<()> = Mutex::new(());

unsafe extern "C" fn changed(_: HostRef, _: ObjectId, _: u32, _: *const Address) -> Status {
    OK
}
unsafe extern "C" fn copy(_: HostRef, _: CfRef, _: *mut CfRef) -> Status {
    UNSUPPORTED
}
unsafe extern "C" fn write(_: HostRef, _: CfRef, _: CfRef) -> Status {
    UNSUPPORTED
}
unsafe extern "C" fn delete(_: HostRef, _: CfRef) -> Status {
    UNSUPPORTED
}
unsafe extern "C" fn request(_: HostRef, _: ObjectId, _: u64, _: *mut c_void) -> Status {
    UNSUPPORTED
}
static TEST_HOST: HostInterface = HostInterface {
    properties_changed: changed,
    copy_from_storage: copy,
    write_to_storage: write,
    delete_from_storage: delete,
    request_configuration: request,
};

fn address(selector: &[u8; 4], scope: u32) -> Address {
    Address {
        selector: code(selector),
        scope,
        element: 0,
    }
}

unsafe fn read_property<T: Copy>(id: ObjectId, selector: &[u8; 4], scope: u32) -> T {
    let driver = driver_ref();
    let mut result = core::mem::MaybeUninit::<T>::uninit();
    let mut bytes = 0;
    assert_eq!(
        ((**driver).get_property)(
            driver,
            id,
            0,
            &address(selector, scope),
            0,
            ptr::null(),
            size_of::<T>() as u32,
            &mut bytes,
            result.as_mut_ptr().cast(),
        ),
        OK
    );
    assert_eq!(bytes, size_of::<T>() as u32);
    result.assume_init()
}

unsafe fn assert_cf_text(value: CfRef, expected: &str) {
    let text = CFStringCreateWithBytes(
        ptr::null(),
        expected.as_ptr(),
        expected.len() as isize,
        0x0800_0100,
        0,
    );
    assert!(!value.is_null() && !text.is_null());
    assert_ne!(CFEqual(value, text), 0);
    CFRelease(text);
}

#[test]
fn sdk_64_bit_layout() {
    assert_eq!(size_of::<DriverInterface>(), 23 * 8);
    assert_eq!(size_of::<HostInterface>(), 5 * 8);
    assert_eq!(size_of::<ClientInfo>(), 24);
    assert_eq!(size_of::<TimeStamp>(), 64);
    assert_eq!(size_of::<CycleInfo>(), 224);
    assert_eq!(size_of::<Format>(), 40);
    assert_eq!(size_of::<RangedFormat>(), 56);
    assert_eq!(size_of::<Uuid>(), 16);
    assert_eq!(offset_of!(DriverInterface, do_io), 21 * 8);
    assert_eq!(offset_of!(CycleInfo, input), 80);
    assert_eq!(offset_of!(CycleInfo, output), 144);
}

#[test]
fn direct_vtable_properties_reject_bad_inputs_and_validate_formats() {
    unsafe {
        let driver = driver_ref();
        let v = &**driver;
        let mut result = ptr::null_mut();
        assert_eq!(
            (v.query_interface)(driver.cast(), DRIVER_UUID, &mut result),
            OK
        );
        assert_eq!(result, driver.cast());
        assert_ne!(
            (v.query_interface)(driver.cast(), TYPE_UUID, &mut result),
            OK
        );
        assert!(result.is_null());
        assert_eq!(
            (v.has_property)(driver, 9999, 0, &address(b"lnam", GLOBAL)),
            0
        );
        assert_eq!(
            (v.has_property)(driver, 100, 0, &address(b"nsrt", INPUT)),
            0
        );
        assert_eq!((v.has_property)(driver, 100, 0, ptr::null()), 0);
        let mut bytes = 999;
        let mut result = [0u32; 2];
        let list = address(b"dev#", GLOBAL);
        assert_eq!(
            (v.get_property_size)(driver, PLUGIN, 0, &list, 0, ptr::null(), &mut bytes),
            OK
        );
        assert_eq!(bytes, 8);
        assert_eq!(
            (v.get_property)(
                driver,
                PLUGIN,
                0,
                &list,
                0,
                ptr::null(),
                7,
                &mut bytes,
                result.as_mut_ptr().cast()
            ),
            BAD_SIZE
        );
        assert_eq!(bytes, 0);
        assert_eq!(
            (v.get_property)(
                driver,
                PLUGIN,
                0,
                &list,
                0,
                ptr::null(),
                8,
                &mut bytes,
                result.as_mut_ptr().cast()
            ),
            OK
        );
        assert_eq!((result, bytes), ([100, 200], 8));
        let mut raw = [0u8; 42];
        let format = address(b"sfmt", GLOBAL);
        assert_eq!(
            (v.get_property)(
                driver,
                101,
                0,
                &format,
                0,
                ptr::null(),
                40,
                &mut bytes,
                raw.as_mut_ptr().add(1).cast()
            ),
            OK
        );
        assert_eq!(
            ptr::read_unaligned(raw.as_ptr().add(1).cast::<Format>()),
            FORMAT
        );
        let mut malformed = FORMAT;
        malformed.channels = 1;
        assert_eq!(
            (v.set_property)(
                driver,
                101,
                0,
                &format,
                0,
                ptr::null(),
                40,
                (&malformed as *const Format).cast()
            ),
            BAD_FORMAT
        );
        assert_eq!(
            (v.set_property)(
                driver,
                101,
                0,
                &format,
                0,
                ptr::null(),
                40,
                (&FORMAT as *const Format).cast()
            ),
            OK
        );
        assert_eq!(
            (v.set_property)(
                driver,
                101,
                0,
                &format,
                0,
                ptr::null(),
                39,
                (&FORMAT as *const Format).cast()
            ),
            BAD_SIZE
        );
        let wrong_class = code(b"adev");
        let owned = address(b"ownd", GLOBAL);
        assert_eq!(
            (v.get_property_size)(
                driver,
                100,
                0,
                &owned,
                4,
                (&wrong_class as *const u32).cast(),
                &mut bytes
            ),
            OK
        );
        assert_eq!(bytes, 0);
    }
}

#[test]
fn direct_host_lifecycle_audio_clock_and_allocation_contract() {
    let _lifecycle = LIFECYCLE.lock().unwrap();
    unsafe {
        let driver = driver_ref();
        let v = &**driver;
        assert_eq!((v.initialize)(driver, &TEST_HOST), OK);
        let client = ClientInfo {
            id: 601,
            process_id: 0,
            native_endian: 1,
            bundle_id: ptr::null(),
        };
        assert_eq!((v.add_client)(driver, 100, &client), OK);
        assert_eq!((v.start_io)(driver, 100, 601), OK);
        let mut cycle = CycleInfo::default();
        cycle.output.sample_time = 1000.0;
        cycle.output.flags = 1;
        cycle.input.sample_time = 1512.0;
        cycle.input.flags = 1;
        let mut source = [0.25f32, -0.25, 0.5, -0.5, 0.75, -0.75, 1.0, -1.0];
        let mut destination = [99.0f32; 8];
        let mut sample = 0.0;
        let mut host = 0;
        let mut seed = 0;
        ALLOCATIONS.with(|n| n.set(0));
        TRACK_ALLOCATIONS.with(|b| b.set(true));
        let write_status = (v.do_io)(
            driver,
            100,
            102,
            601,
            WRITE,
            4,
            &cycle,
            source.as_mut_ptr().cast(),
            ptr::null_mut(),
        );
        let read_status = (v.do_io)(
            driver,
            100,
            101,
            601,
            READ,
            4,
            &cycle,
            destination.as_mut_ptr().cast(),
            ptr::null_mut(),
        );
        let time_status = (v.zero_timestamp)(driver, 100, 601, &mut sample, &mut host, &mut seed);
        TRACK_ALLOCATIONS.with(|b| b.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        assert_eq!((write_status, read_status, time_status), (OK, OK, OK));
        assert_eq!(destination, source);
        assert_eq!(seed, 1);
        assert!(host <= mach_absolute_time());
        assert_eq!(
            (v.do_io)(
                driver,
                100,
                102,
                601,
                READ,
                4,
                &cycle,
                destination.as_mut_ptr().cast(),
                ptr::null_mut()
            ),
            BAD_STREAM
        );
        assert_eq!(
            (v.do_io)(
                driver,
                100,
                101,
                601,
                READ,
                8193,
                &cycle,
                destination.as_mut_ptr().cast(),
                ptr::null_mut()
            ),
            ILLEGAL
        );
        cycle.input.sample_time = 2000.0;
        assert_eq!(
            (v.do_io)(
                driver,
                100,
                101,
                601,
                READ,
                4,
                &cycle,
                destination.as_mut_ptr().cast(),
                ptr::null_mut()
            ),
            OK
        );
        assert_eq!(destination, [0.0; 8]);
        cycle.input.sample_time = f64::NAN;
        assert_eq!(
            (v.do_io)(
                driver,
                100,
                101,
                601,
                READ,
                4,
                &cycle,
                destination.as_mut_ptr().cast(),
                ptr::null_mut()
            ),
            ILLEGAL
        );
        assert_eq!((v.stop_io)(driver, 100, 601), OK);
        assert_eq!((v.stop_io)(driver, 100, 601), ILLEGAL);
        assert_eq!((v.remove_client)(driver, 100, &client), OK);
    }
}

#[test]
fn published_cables_have_independent_clients_audio_and_restart_state() {
    let _lifecycle = LIFECYCLE.lock().unwrap();
    unsafe {
        let driver = driver_ref();
        let v = &**driver;
        assert_eq!((v.initialize)(driver, &TEST_HOST), OK);
        assert_eq!(
            read_property::<[u32; 2]>(PLUGIN, b"ownd", GLOBAL),
            [100, 200]
        );
        for (id, expected_name, expected_uid) in [
            (100, "ReLink", "au.textual.ravelink.cable.1"),
            (200, "ReLink 2", "au.textual.ravelink.cable.2"),
        ] {
            let name = read_property::<CfRef>(id, b"lnam", GLOBAL);
            assert_cf_text(name, expected_name);
            CFRelease(name);
            let uid = read_property::<CfRef>(id, b"uid ", GLOBAL);
            assert_cf_text(uid, expected_uid);
            let mut translated = 0;
            let mut bytes = 0;
            assert_eq!(
                (v.get_property)(
                    driver,
                    PLUGIN,
                    0,
                    &address(b"uidd", GLOBAL),
                    size_of::<CfRef>() as u32,
                    (&uid as *const CfRef).cast(),
                    4,
                    &mut bytes,
                    (&mut translated as *mut u32).cast(),
                ),
                OK
            );
            assert_eq!((translated, bytes), (id, 4));
            CFRelease(uid);
            assert_eq!(
                read_property::<[u32; 2]>(id, b"stm#", GLOBAL),
                [id + 1, id + 2]
            );
            assert_eq!(read_property::<u32>(id, b"stm#", INPUT), id + 1);
            assert_eq!(read_property::<u32>(id, b"stm#", OUTPUT), id + 2);
            for stream in [id + 1, id + 2] {
                assert_eq!(read_property::<u32>(stream, b"stdv", GLOBAL), id);
                assert_eq!(read_property::<Format>(stream, b"sfmt", GLOBAL), FORMAT);
            }
        }

        // The host may reuse the same client ID on separate devices.
        let client = ClientInfo {
            id: 702,
            process_id: 0,
            native_endian: 1,
            bundle_id: ptr::null(),
        };
        for id in [100, 200] {
            assert_eq!((v.add_client)(driver, id, &client), OK);
            assert_eq!((v.start_io)(driver, id, client.id), OK);
        }
        let extra = ClientInfo { id: 703, ..client };
        assert_eq!((v.add_client)(driver, 200, &extra), OK);
        assert_eq!((v.start_io)(driver, 100, extra.id), ILLEGAL);
        let mut cycle = CycleInfo::default();
        cycle.output.sample_time = 19_000.0;
        cycle.output.flags = 1;
        cycle.input.sample_time = 19_512.0;
        cycle.input.flags = 1;
        let mut a = [0.125f32, -0.25, 0.375, -0.5];
        let mut b = [-0.75f32, 0.625, -0.5, 0.375];
        let mut captured = [0.0f32; 4];
        let io = |id: u32, stream: u32, op, samples: &mut [f32; 4]| {
            (v.do_io)(
                driver,
                id,
                stream,
                client.id,
                op,
                2,
                &cycle,
                samples.as_mut_ptr().cast(),
                ptr::null_mut(),
            )
        };
        ALLOCATIONS.with(|n| n.set(0));
        TRACK_ALLOCATIONS.with(|b| b.set(true));
        assert_eq!(io(100, 102, WRITE, &mut a), OK);
        assert_eq!(io(200, 202, WRITE, &mut b), OK);
        assert_eq!(io(100, 101, READ, &mut captured), OK);
        assert_eq!(captured, a);
        assert_eq!(io(200, 201, READ, &mut captured), OK);
        assert_eq!(captured, b);
        assert_eq!(io(100, 201, READ, &mut captured), BAD_STREAM);
        TRACK_ALLOCATIONS.with(|b| b.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);

        // Adding and stopping a second client does not reset an active cable.
        assert_eq!((v.start_io)(driver, 200, extra.id), OK);
        assert_eq!(io(200, 201, READ, &mut captured), OK);
        assert_eq!(captured, b);
        assert_eq!((v.stop_io)(driver, 200, extra.id), OK);
        assert_eq!((v.remove_client)(driver, 200, &extra), OK);
        assert_eq!(read_property::<u32>(200, b"goin", GLOBAL), 1);

        // Restarting either device clears only its own ring and running state.
        for (reset, other, other_samples) in [(100, 200, b), (200, 100, a)] {
            assert_eq!((v.stop_io)(driver, reset, client.id), OK);
            assert_eq!(read_property::<u32>(reset, b"goin", GLOBAL), 0);
            assert_eq!(read_property::<u32>(other, b"goin", GLOBAL), 1);
            assert_eq!(io(other, other + 1, READ, &mut captured), OK);
            assert_eq!(captured, other_samples);
            assert_eq!((v.start_io)(driver, reset, client.id), OK);
            assert_eq!(io(reset, reset + 1, READ, &mut captured), OK);
            assert_eq!(captured, [0.0; 4]);
            assert_eq!(io(other, other + 1, READ, &mut captured), OK);
            assert_eq!(captured, other_samples);
            let mut restore = if reset == 100 { a } else { b };
            assert_eq!(io(reset, reset + 2, WRITE, &mut restore), OK);
        }
        for id in [100, 200] {
            assert_eq!((v.stop_io)(driver, id, client.id), OK);
            assert_eq!((v.remove_client)(driver, id, &client), OK);
        }
    }
}
