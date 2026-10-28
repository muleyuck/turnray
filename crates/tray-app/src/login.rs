//! Launch at login, through `SMAppService.mainApp`. It registers the `.app` the running
//! binary is in, so it is offered only from a bundle and only where the class exists.

use objc2_foundation::{NSBundle, NSOperatingSystemVersion, NSProcessInfo};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// macOS 13 added `SMAppService`. A bare binary (`target/release/turnray`) has no bundle
/// identifier, and the service reports `NotFound` for it.
pub fn available() -> bool {
    let ventura = NSOperatingSystemVersion {
        majorVersion: 13,
        minorVersion: 0,
        patchVersion: 0,
    };
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(ventura)
        && NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// `None` where launch at login isn't offered (`available`).
pub fn enabled() -> Option<bool> {
    available().then(|| is_on(unsafe { SMAppService::mainAppService().status() }))
}

/// Never registered reads `NotFound`, not `NotRegistered`. `RequiresApproval` is
/// registered but switched off in System Settings, so it is off too.
fn is_on(status: SMAppServiceStatus) -> bool {
    status == SMAppServiceStatus::Enabled
}

/// Registers or unregisters the app. A failure is logged; the caller reads `enabled`
/// back, so the checkbox shows what macOS ended up with.
pub fn set(on: bool) {
    let service = unsafe { SMAppService::mainAppService() };
    let result = if on {
        unsafe { service.registerAndReturnError() }
    } else {
        unsafe { service.unregisterAndReturnError() }
    };
    if let Err(e) = result {
        tracing::warn!(
            "{} launch at login failed: {}",
            if on { "turning on" } else { "turning off" },
            e.localizedDescription()
        );
    }
    // Only System Settings can approve it, so that is where to go.
    if on && unsafe { service.status() } == SMAppServiceStatus::RequiresApproval {
        unsafe { SMAppService::openSystemSettingsLoginItems() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_enabled_is_on() {
        assert!(is_on(SMAppServiceStatus::Enabled));
        for off in [
            SMAppServiceStatus::NotRegistered,
            SMAppServiceStatus::RequiresApproval,
            SMAppServiceStatus::NotFound,
        ] {
            assert!(!is_on(off));
        }
    }
}
