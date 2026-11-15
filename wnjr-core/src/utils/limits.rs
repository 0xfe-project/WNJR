use crate::WasmError;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    min: usize,
    max: Option<usize>,
    default_max: Option<usize>,
    pub(crate) is64: bool,
    /// True when the memory limits flag included the shared bit (0x02/0x03).
    pub(crate) shared: bool,
}

impl Limits {
    pub fn new(min: usize, max: Option<usize>) -> Result<Self, WasmError> {
        if let Some(max) = max {
            if max < min {
                return Err(WasmError::invalid("min larger than max"));
            }
        }
        Ok(Limits {
            min,
            max,
            default_max: None,
            is64: false,
            shared: false,
        })
    }

    pub fn new_64(min: usize, max: Option<usize>) -> Result<Self, WasmError> {
        if let Some(max) = max {
            if max < min {
                return Err(WasmError::invalid("min larger than max"));
            }
        }
        Ok(Limits {
            min,
            max,
            default_max: None,
            is64: true,
            shared: false,
        })
    }

    /// Create limits for a shared 32-bit memory. Requires max.
    ///
    /// Shared memories are the ones the threads proposal operates on: their
    /// address space is reserved once at creation, so every instance that
    /// imports the same SharedMemHandle sees the same bytes.
    pub fn new_shared(min: usize, max: usize) -> Result<Self, WasmError> {
        if max < min {
            return Err(WasmError::invalid("min larger than max"));
        }
        Ok(Limits {
            min,
            max: Some(max),
            default_max: None,
            is64: false,
            shared: true,
        })
    }

    /// Create limits for a shared 64-bit memory. Requires max.
    pub(crate) fn new_shared_64(min: usize, max: usize) -> Result<Self, WasmError> {
        if max < min {
            return Err(WasmError::invalid("min larger than max"));
        }
        Ok(Limits {
            min,
            max: Some(max),
            default_max: None,
            is64: true,
            shared: true,
        })
    }

    /// Whether this memory or table is shared (threads proposal).
    pub fn is_shared(&self) -> bool {
        self.shared
    }

    /// Whether these limits describe a 64-bit memory or table.
    pub fn is_64(&self) -> bool {
        self.is64
    }

    /// Returns the minimum value.
    pub fn min(&self) -> usize {
        self.min
    }

    /// Returns the explicit maximum value, if any.
    pub fn max(&self) -> Option<usize> {
        self.max
    }

    /// Returns the effective maximum (explicit max or default max).
    pub(crate) fn effective_max(&self) -> usize {
        self.max
            .unwrap_or_else(|| self.default_max.unwrap_or(usize::MAX))
    }

    pub(crate) fn with_default_max(&self, default_max: usize) -> Result<Self, WasmError> {
        if let Some(max) = self.max {
            if max > default_max {
                return Err(WasmError::invalid("max larger than default max"));
            }
        }
        if self.min > default_max {
            return Err(WasmError::invalid("min larger than default max"));
        }
        Ok(Limits {
            min: self.min,
            max: self.max,
            default_max: Some(default_max),
            is64: self.is64,
            shared: self.shared,
        })
    }
}

pub(crate) trait Limitable {
    fn limits(&self) -> &Limits;
}
