mod domain;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::RemappingService;

#[cfg(test)]
mod tests;
