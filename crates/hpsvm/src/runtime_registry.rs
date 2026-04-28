use std::fmt;

use crate::CustomSyscallRegistration;

#[derive(Clone, Default)]
pub(crate) struct RuntimeExtensionRegistry {
    custom_syscalls: Vec<CustomSyscallRegistration>,
    #[cfg(feature = "precompiles")]
    load_standard_precompiles: bool,
}

impl RuntimeExtensionRegistry {
    pub(crate) fn custom_syscalls(&self) -> &[CustomSyscallRegistration] {
        &self.custom_syscalls
    }

    pub(crate) fn register_custom_syscall(&mut self, registration: CustomSyscallRegistration) {
        self.custom_syscalls.push(registration);
    }

    #[cfg(feature = "precompiles")]
    pub(crate) fn enable_standard_precompiles(&mut self) {
        self.load_standard_precompiles = true;
    }

    #[cfg(feature = "precompiles")]
    pub(crate) const fn loads_standard_precompiles(&self) -> bool {
        self.load_standard_precompiles
    }
}

impl fmt::Debug for RuntimeExtensionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("RuntimeExtensionRegistry");
        debug.field("custom_syscall_count", &self.custom_syscalls.len());
        #[cfg(feature = "precompiles")]
        debug.field("load_standard_precompiles", &self.load_standard_precompiles);
        debug.finish()
    }
}
