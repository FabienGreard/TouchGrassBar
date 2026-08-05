#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;

    const AF_INET: u8 = 2;
    const REACHABLE: u32 = 1 << 1;
    const CONNECTION_REQUIRED: u32 = 1 << 2;
    const CONNECTION_ON_TRAFFIC: u32 = 1 << 3;
    const INTERVENTION_REQUIRED: u32 = 1 << 4;
    const CONNECTION_ON_DEMAND: u32 = 1 << 5;

    #[repr(C)]
    struct InAddress {
        address: u32,
    }

    #[repr(C)]
    struct SocketAddressV4 {
        length: u8,
        family: u8,
        port: u16,
        address: InAddress,
        zero: [u8; 8],
    }

    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCNetworkReachabilityCreateWithAddress(
            allocator: *const c_void,
            address: *const c_void,
        ) -> *const c_void;
        fn SCNetworkReachabilityGetFlags(target: *const c_void, flags: *mut u32) -> u8;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }

    pub fn is_reachable() -> Option<bool> {
        let address = SocketAddressV4 {
            length: u8::try_from(size_of::<SocketAddressV4>()).ok()?,
            family: AF_INET,
            port: 0,
            address: InAddress { address: 0 },
            zero: [0; 8],
        };

        // A zero IPv4 address asks SystemConfiguration for the current default
        // route. This call does not send provider or application data.
        let target = unsafe {
            SCNetworkReachabilityCreateWithAddress(
                std::ptr::null(),
                (&raw const address).cast::<c_void>(),
            )
        };
        if target.is_null() {
            return None;
        }

        let mut flags = 0;
        let succeeded = unsafe { SCNetworkReachabilityGetFlags(target, &raw mut flags) } != 0;
        unsafe { CFRelease(target) };
        if !succeeded {
            return None;
        }

        let reachable = flags & REACHABLE != 0;
        let connection_required = flags & CONNECTION_REQUIRED != 0;
        let can_connect_automatically = flags & (CONNECTION_ON_DEMAND | CONNECTION_ON_TRAFFIC) != 0;
        let intervention_required = flags & INTERVENTION_REQUIRED != 0;
        Some(
            reachable
                && (!connection_required || (can_connect_automatically && !intervention_required)),
        )
    }
}

#[cfg(target_os = "macos")]
pub use platform::is_reachable;

#[cfg(not(target_os = "macos"))]
pub fn is_reachable() -> Option<bool> {
    None
}
