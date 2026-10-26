use crate::{abi::*, device, valid_driver};
use core::{ffi::c_void, mem::size_of, ptr, sync::atomic::Ordering};

// Keep property list writes bounded if the static device registry grows.
const _: () = assert!(device::DEVICES.len() <= 8);

enum Value {
    U32(u32),
    F64(f64),
    Text(&'static str),
    Ids([u32; 8], usize),
    Format,
    Rates,
    RangedFormat,
    Translate,
}
impl Value {
    fn size(&self) -> u32 {
        match self {
            Self::U32(_) | Self::Translate => 4,
            Self::F64(_) => 8,
            Self::Text(_) => size_of::<CfRef>() as u32,
            Self::Ids(_, len) => *len as u32 * 4,
            Self::Format => size_of::<Format>() as u32,
            Self::Rates => size_of::<Range>() as u32,
            Self::RangedFormat => size_of::<RangedFormat>() as u32,
        }
    }
}
fn ids(values: &[u32]) -> Value {
    let mut array = [0; 8];
    array[..values.len()].copy_from_slice(values);
    Value::Ids(array, values.len())
}
fn class(id: ObjectId) -> u32 {
    if id == PLUGIN {
        code(b"aplg")
    } else if device::find(id).is_some() {
        code(b"adev")
    } else {
        code(b"astr")
    }
}
fn value(id: ObjectId, a: &Address) -> Result<Value, Status> {
    let parent = device::owner(id);
    if id != PLUGIN && parent.is_none() {
        return Err(BAD_OBJECT);
    }
    if a.element != 0 {
        return Err(UNKNOWN_PROPERTY);
    }
    let is_device = parent.is_some_and(|d| d.id == id);
    let scoped = is_device && matches!(a.scope, INPUT | OUTPUT);
    if a.scope != GLOBAL && !scoped {
        return Err(UNKNOWN_PROPERTY);
    }
    let common = match a.selector {
        s if s == code(b"bcls") => Some(Value::U32(code(b"aobj"))),
        s if s == code(b"clas") => Some(Value::U32(class(id))),
        s if s == code(b"stdv") => Some(Value::U32(if id == PLUGIN {
            0
        } else if is_device {
            PLUGIN
        } else {
            parent.unwrap().id
        })),
        s if s == code(b"lnam") => Some(Value::Text(if id == PLUGIN {
            "ReLink Audio"
        } else if is_device {
            parent.unwrap().name
        } else if parent.unwrap().input == id {
            "ReLink Capture"
        } else {
            "ReLink Playback"
        })),
        s if s == code(b"lmak") => Some(Value::Text("Textual")),
        s if s == code(b"ctrl") || s == code(b"cust") => Some(ids(&[])),
        _ => None,
    };
    if let Some(v) = common {
        return if a.scope == GLOBAL {
            Ok(v)
        } else {
            Err(UNKNOWN_PROPERTY)
        };
    }
    if id == PLUGIN {
        return match a.selector {
            s if s == code(b"piid") => Ok(Value::Text("au.textual.ravelink.hal")),
            s if s == code(b"rsrc") => Ok(Value::Text("")),
            s if s == code(b"box#") || s == code(b"clk#") => Ok(ids(&[])),
            s if s == code(b"uidd") => Ok(Value::Translate),
            s if s == code(b"ownd") || s == code(b"dev#") => {
                let mut list = [0; 8];
                for (i, d) in device::DEVICES.iter().enumerate() {
                    list[i] = d.id;
                }
                Ok(Value::Ids(list, device::DEVICES.len()))
            }
            _ => Err(UNKNOWN_PROPERTY),
        };
    }
    let d = parent.unwrap();
    if is_device {
        return match a.selector {
            s if s == code(b"stm#") || s == code(b"ownd") => Ok(match a.scope {
                INPUT => ids(&[d.input]),
                OUTPUT => ids(&[d.output]),
                _ => ids(&[d.input, d.output]),
            }),
            s if s == code(b"dflt") || s == code(b"sflt") => {
                if scoped {
                    Ok(Value::U32(1))
                } else {
                    Err(UNKNOWN_PROPERTY)
                }
            }
            s if s == code(b"ltnc") => {
                if scoped {
                    Ok(Value::U32(if a.scope == INPUT {
                        device::LOOPBACK_DELAY as u32
                    } else {
                        0
                    }))
                } else {
                    Err(UNKNOWN_PROPERTY)
                }
            }
            s if s == code(b"saft") => {
                if scoped {
                    Ok(Value::U32(0))
                } else {
                    Err(UNKNOWN_PROPERTY)
                }
            }
            s if s == code(b"dch2") => {
                if scoped {
                    Ok(ids(&[1, 2]))
                } else {
                    Err(UNKNOWN_PROPERTY)
                }
            }
            _ if a.scope != GLOBAL => Err(UNKNOWN_PROPERTY),
            s if s == code(b"uid ") => Ok(Value::Text(d.uid)),
            s if s == code(b"muid") => Ok(Value::Text("au.textual.ravelink.stereo.48000")),
            s if s == code(b"lmod") => Ok(Value::Text("ReLink Stereo Cable")),
            s if s == code(b"tran") => Ok(Value::U32(code(b"virt"))),
            s if s == code(b"akin") => Ok(ids(&[d.id])),
            s if s == code(b"clkd") || s == code(b"hidn") => Ok(Value::U32(0)),
            s if s == code(b"livn") || s == code(b"cstb") => Ok(Value::U32(1)),
            s if s == code(b"goin") => Ok(Value::U32(u32::from(
                d.running.load(Ordering::Acquire) != 0,
            ))),
            s if s == code(b"nsrt") => Ok(Value::F64(FORMAT.sample_rate)),
            s if s == code(b"nsr#") => Ok(Value::Rates),
            s if s == code(b"ring") => Ok(Value::U32(device::PERIOD as u32)),
            s if s == code(b"clok") => Ok(Value::U32(code(b"raww"))),
            _ => Err(UNKNOWN_PROPERTY),
        };
    }
    match a.selector {
        s if s == code(b"ownd") => Ok(ids(&[])),
        s if s == code(b"sact") || s == code(b"schn") => Ok(Value::U32(1)),
        s if s == code(b"sdir") => Ok(Value::U32(u32::from(id == d.input))),
        s if s == code(b"term") => Ok(Value::U32(code(b"line"))),
        s if s == code(b"ltnc") => Ok(Value::U32(0)),
        s if s == code(b"sfmt") || s == code(b"pft ") => Ok(Value::Format),
        s if s == code(b"sfma") || s == code(b"pfta") => Ok(Value::RangedFormat),
        _ => Err(UNKNOWN_PROPERTY),
    }
}

unsafe fn address(driver: DriverRef, raw: *const Address) -> Result<Address, Status> {
    if !valid_driver(driver) {
        return Err(BAD_OBJECT);
    }
    if raw.is_null() {
        return Err(ILLEGAL);
    }
    Ok(ptr::read_unaligned(raw))
}
fn qualifier(v: &Value, size: u32, data: *const c_void) -> Result<(), Status> {
    match v {
        Value::Translate if size != size_of::<CfRef>() as u32 || data.is_null() => Err(BAD_SIZE),
        Value::Translate => Ok(()),
        // OwnedObjects accepts optional class-ID qualifiers; handle these separately below.
        _ if size != 0 => Err(BAD_SIZE),
        _ => Ok(()),
    }
}
unsafe fn property(
    driver: DriverRef,
    id: ObjectId,
    raw: *const Address,
    qsize: u32,
    qdata: *const c_void,
) -> Result<Value, Status> {
    let a = address(driver, raw)?;
    let mut result = value(id, &a)?;
    if a.selector == code(b"ownd") && qsize != 0 {
        if qsize % 4 != 0 || qdata.is_null() {
            return Err(BAD_SIZE);
        }
        if let Value::Ids(ref mut list, ref mut length) = result {
            let mut kept = 0;
            for i in 0..*length {
                let object = list[i];
                let mut matched = false;
                for q in 0..qsize / 4 {
                    let wanted = ptr::read_unaligned(qdata.cast::<u32>().add(q as usize));
                    matched |= wanted == class(object) || wanted == code(b"aobj");
                }
                if matched {
                    list[kept] = object;
                    kept += 1;
                }
            }
            *length = kept;
        }
    } else {
        qualifier(&result, qsize, qdata)?;
    }
    Ok(result)
}
pub unsafe extern "C" fn has(driver: DriverRef, id: ObjectId, _: i32, raw: *const Address) -> u8 {
    u8::from(address(driver, raw).and_then(|a| value(id, &a)).is_ok())
}
pub unsafe extern "C" fn settable(
    driver: DriverRef,
    id: ObjectId,
    _: i32,
    raw: *const Address,
    out: *mut u8,
) -> Status {
    if out.is_null() {
        return ILLEGAL;
    }
    let a = match address(driver, raw) {
        Ok(a) => a,
        Err(e) => return e,
    };
    if let Err(e) = value(id, &a) {
        return e;
    }
    // A fixed format can be reasserted, but no configuration change is advertised.
    ptr::write(out, 0);
    OK
}
pub unsafe extern "C" fn size(
    driver: DriverRef,
    id: ObjectId,
    _: i32,
    raw: *const Address,
    qsize: u32,
    qdata: *const c_void,
    out: *mut u32,
) -> Status {
    if out.is_null() {
        return ILLEGAL;
    }
    match property(driver, id, raw, qsize, qdata) {
        Ok(v) => {
            ptr::write_unaligned(out, v.size());
            OK
        }
        Err(e) => e,
    }
}
unsafe fn put<T: Copy>(out: *mut c_void, value: T) {
    ptr::write_unaligned(out.cast::<T>(), value);
}
unsafe fn text(value: &str) -> CfRef {
    CFStringCreateWithBytes(
        ptr::null(),
        value.as_ptr(),
        value.len() as isize,
        0x0800_0100,
        0,
    )
}
pub unsafe extern "C" fn get(
    driver: DriverRef,
    id: ObjectId,
    _: i32,
    raw: *const Address,
    qsize: u32,
    qdata: *const c_void,
    capacity: u32,
    written: *mut u32,
    out: *mut c_void,
) -> Status {
    if written.is_null() {
        return ILLEGAL;
    }
    ptr::write_unaligned(written, 0);
    let v = match property(driver, id, raw, qsize, qdata) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let required = v.size();
    if capacity < required {
        return BAD_SIZE;
    }
    if required != 0 && out.is_null() {
        return ILLEGAL;
    }
    match v {
        Value::U32(n) => put(out, n),
        Value::F64(n) => put(out, n),
        Value::Text(s) => {
            let string = text(s);
            if string.is_null() {
                return ILLEGAL;
            }
            put(out, string);
        }
        Value::Ids(list, len) => {
            for (i, id) in list.iter().take(len).enumerate() {
                ptr::write_unaligned(out.cast::<u32>().add(i), *id);
            }
        }
        Value::Format => put(out, FORMAT),
        Value::Rates => put(
            out,
            Range {
                minimum: FORMAT.sample_rate,
                maximum: FORMAT.sample_rate,
            },
        ),
        Value::RangedFormat => put(
            out,
            RangedFormat {
                format: FORMAT,
                range: Range {
                    minimum: FORMAT.sample_rate,
                    maximum: FORMAT.sample_rate,
                },
            },
        ),
        Value::Translate => {
            let requested = ptr::read_unaligned(qdata.cast::<CfRef>());
            if requested.is_null() {
                return ILLEGAL;
            }
            let mut found = 0;
            for d in &device::DEVICES {
                let uid = text(d.uid);
                if uid.is_null() {
                    return ILLEGAL;
                }
                let equal = CFEqual(requested, uid) != 0;
                CFRelease(uid);
                if equal {
                    found = d.id;
                    break;
                }
            }
            put(out, found);
        }
    }
    ptr::write_unaligned(written, required);
    OK
}
pub unsafe extern "C" fn set(
    driver: DriverRef,
    id: ObjectId,
    _: i32,
    raw: *const Address,
    qsize: u32,
    qdata: *const c_void,
    size: u32,
    data: *const c_void,
) -> Status {
    let v = match property(driver, id, raw, qsize, qdata) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if size != v.size() {
        return BAD_SIZE;
    }
    if size != 0 && data.is_null() {
        return ILLEGAL;
    }
    // Validation is explicit even for unsupported mutations: malformed ASBDs are
    // never accepted. Exact fixed-format assertions are harmless no-ops.
    match v {
        Value::Format => {
            if ptr::read_unaligned(data.cast::<Format>()) == FORMAT {
                OK
            } else {
                BAD_FORMAT
            }
        }
        Value::F64(_) if ptr::read_unaligned(raw).selector == code(b"nsrt") => {
            if ptr::read_unaligned(data.cast::<f64>()) == FORMAT.sample_rate {
                OK
            } else {
                BAD_FORMAT
            }
        }
        _ => UNSUPPORTED,
    }
}
