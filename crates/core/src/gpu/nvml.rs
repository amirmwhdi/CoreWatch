//! NVIDIA's management library (NVML), for the proprietary driver.
//!
//! The proprietary driver puts nothing about load, memory, clocks or
//! temperature in sysfs; `nvidia-smi` and every monitor read them through
//! `libnvidia-ml.so.1`, which comes with the driver (and with the NVIDIA GL
//! extension inside Flatpak). It is opened at run time, so Corewatch neither
//! links to it nor needs it on other machines.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};

type Device = *mut c_void;
type Ret = c_int;
const SUCCESS: Ret = 0;
const CLOCK_GRAPHICS: c_int = 0;
const CLOCK_MEM: c_int = 2;
const TEMPERATURE_GPU: c_int = 0;

/// What NVML reports for one card in one tick; each value is `None` when
/// the card or driver does not support it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct NvmlReading {
    pub usage: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub freq_mhz: Option<u32>,
    pub max_freq_mhz: Option<u32>,
    pub mem_freq_mhz: Option<u32>,
    pub mem_max_freq_mhz: Option<u32>,
    pub temp_c: Option<u32>,
    pub power_w: Option<f32>,
    pub power_cap_w: Option<f32>,
    pub fan_percent: Option<f32>,
}

#[allow(non_snake_case)]
struct Api {
    nvmlShutdown: unsafe extern "C" fn() -> Ret,
    nvmlDeviceGetHandleByPciBusId_v2: unsafe extern "C" fn(*const c_char, *mut Device) -> Ret,
    nvmlDeviceGetName: unsafe extern "C" fn(Device, *mut c_char, c_uint) -> Ret,
    nvmlDeviceGetUtilizationRates: unsafe extern "C" fn(Device, *mut [c_uint; 2]) -> Ret,
    nvmlDeviceGetMemoryInfo: unsafe extern "C" fn(Device, *mut [u64; 3]) -> Ret,
    nvmlDeviceGetClockInfo: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Ret,
    nvmlDeviceGetMaxClockInfo: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Ret,
    nvmlDeviceGetTemperature: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Ret,
    nvmlDeviceGetPowerUsage: unsafe extern "C" fn(Device, *mut c_uint) -> Ret,
    nvmlDeviceGetEnforcedPowerLimit: unsafe extern "C" fn(Device, *mut c_uint) -> Ret,
    nvmlDeviceGetFanSpeed: unsafe extern "C" fn(Device, *mut c_uint) -> Ret,
}

/// An initialized NVML. Dropping it shuts NVML down and closes the library.
pub struct Nvml {
    lib: *mut c_void,
    api: Api,
}

// SAFETY: NVML is thread-safe; the handle is only used from the sampler thread.
unsafe impl Send for Nvml {}

impl Nvml {
    /// `None` when the library is missing (no proprietary driver) or does
    /// not start (driver and library versions differ).
    // Each symbol's type comes from the `Api` field it is stored in.
    #[allow(clippy::missing_transmute_annotations)]
    pub fn open() -> Option<Self> {
        // SAFETY: dlopen with a constant, NUL-terminated name.
        let lib = unsafe { libc::dlopen(c"libnvidia-ml.so.1".as_ptr(), libc::RTLD_NOW) };
        if lib.is_null() {
            return None;
        }
        // SAFETY: each symbol is cast to the signature NVML documents for it.
        let api = unsafe {
            macro_rules! sym {
                ($name:literal) => {{
                    let ptr = libc::dlsym(lib, concat!($name, "\0").as_ptr().cast());
                    if ptr.is_null() {
                        libc::dlclose(lib);
                        return None;
                    }
                    std::mem::transmute::<*mut c_void, _>(ptr)
                }};
            }
            let init: unsafe extern "C" fn() -> Ret = sym!("nvmlInit_v2");
            if init() != SUCCESS {
                libc::dlclose(lib);
                return None;
            }
            Api {
                nvmlShutdown: sym!("nvmlShutdown"),
                nvmlDeviceGetHandleByPciBusId_v2: sym!("nvmlDeviceGetHandleByPciBusId_v2"),
                nvmlDeviceGetName: sym!("nvmlDeviceGetName"),
                nvmlDeviceGetUtilizationRates: sym!("nvmlDeviceGetUtilizationRates"),
                nvmlDeviceGetMemoryInfo: sym!("nvmlDeviceGetMemoryInfo"),
                nvmlDeviceGetClockInfo: sym!("nvmlDeviceGetClockInfo"),
                nvmlDeviceGetMaxClockInfo: sym!("nvmlDeviceGetMaxClockInfo"),
                nvmlDeviceGetTemperature: sym!("nvmlDeviceGetTemperature"),
                nvmlDeviceGetPowerUsage: sym!("nvmlDeviceGetPowerUsage"),
                nvmlDeviceGetEnforcedPowerLimit: sym!("nvmlDeviceGetEnforcedPowerLimit"),
                nvmlDeviceGetFanSpeed: sym!("nvmlDeviceGetFanSpeed"),
            }
        };
        Some(Self { lib, api })
    }

    /// The card at PCI address `slot` (`"0000:01:00.0"`).
    pub fn device(&self, slot: &str) -> Option<NvmlDevice> {
        let slot = CString::new(slot).ok()?;
        let mut device: Device = std::ptr::null_mut();
        // SAFETY: valid C string and out pointer.
        let ret =
            unsafe { (self.api.nvmlDeviceGetHandleByPciBusId_v2)(slot.as_ptr(), &mut device) };
        (ret == SUCCESS && !device.is_null()).then_some(NvmlDevice(device))
    }

    pub fn name(&self, device: &NvmlDevice) -> Option<String> {
        let mut buf = [0 as c_char; 96];
        // SAFETY: the buffer length is passed along.
        let ret = unsafe {
            (self.api.nvmlDeviceGetName)(device.0, buf.as_mut_ptr(), buf.len() as c_uint)
        };
        if ret != SUCCESS {
            return None;
        }
        // SAFETY: NVML NUL-terminates within the given length.
        let name = unsafe { CStr::from_ptr(buf.as_ptr()) };
        Some(name.to_string_lossy().trim().to_owned())
    }

    /// Everything the card reports now. Unsupported values stay `None`.
    pub fn read(&self, device: &NvmlDevice) -> NvmlReading {
        let d = device.0;
        let api = &self.api;
        let u32_of = |f: &dyn Fn(*mut c_uint) -> Ret| {
            let mut value: c_uint = 0;
            (f(&mut value) == SUCCESS).then_some(value)
        };
        // SAFETY (all calls below): a valid device handle and out pointers.
        let mut util = [0 as c_uint; 2];
        let usage = (unsafe { (api.nvmlDeviceGetUtilizationRates)(d, &mut util) } == SUCCESS)
            .then(|| (util[0] as f32 / 100.0).clamp(0.0, 1.0));
        let mut mem = [0u64; 3]; // total, free, used
        let mem_ok = unsafe { (api.nvmlDeviceGetMemoryInfo)(d, &mut mem) } == SUCCESS;
        let clock = |kind| u32_of(&|v| unsafe { (api.nvmlDeviceGetClockInfo)(d, kind, v) });
        let max_clock = |kind| u32_of(&|v| unsafe { (api.nvmlDeviceGetMaxClockInfo)(d, kind, v) });
        NvmlReading {
            usage,
            vram_used: mem_ok.then_some(mem[2]),
            vram_total: mem_ok.then_some(mem[0]).filter(|t| *t > 0),
            freq_mhz: clock(CLOCK_GRAPHICS),
            max_freq_mhz: max_clock(CLOCK_GRAPHICS).filter(|m| *m > 0),
            mem_freq_mhz: clock(CLOCK_MEM),
            mem_max_freq_mhz: max_clock(CLOCK_MEM).filter(|m| *m > 0),
            temp_c: u32_of(&|v| unsafe { (api.nvmlDeviceGetTemperature)(d, TEMPERATURE_GPU, v) }),
            power_w: u32_of(&|v| unsafe { (api.nvmlDeviceGetPowerUsage)(d, v) })
                .map(|mw| mw as f32 / 1000.0),
            power_cap_w: u32_of(&|v| unsafe { (api.nvmlDeviceGetEnforcedPowerLimit)(d, v) })
                .map(|mw| mw as f32 / 1000.0)
                .filter(|w| *w > 0.0),
            fan_percent: u32_of(&|v| unsafe { (api.nvmlDeviceGetFanSpeed)(d, v) })
                .map(|p| (p as f32 / 100.0).clamp(0.0, 1.0)),
        }
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // SAFETY: NVML was initialized in `open`; the library stays open
        // until after the shutdown call.
        unsafe {
            (self.api.nvmlShutdown)();
            libc::dlclose(self.lib);
        }
    }
}

/// A card handle from [`Nvml::device`]; valid while the `Nvml` lives.
pub struct NvmlDevice(Device);

// SAFETY: see `Nvml`.
unsafe impl Send for NvmlDevice {}
