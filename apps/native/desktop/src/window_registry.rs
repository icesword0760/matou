use std::collections::VecDeque;

use crate::instance_open::OpenRequest;

#[derive(Default)]
pub struct WindowRegistry {
    pending: VecDeque<String>,
    focus: bool,
    reopen: bool,
}

impl WindowRegistry {
    pub fn enqueue(&mut self, request: OpenRequest) {
        let paths = match request {
            OpenRequest::SecondLaunch(paths) => {
                self.focus = true;
                paths
            }
            OpenRequest::Directories(paths) => paths,
            OpenRequest::Reopen => {
                self.reopen = true;
                return;
            }
        };
        for path in paths {
            if !self.pending.contains(&path) {
                self.pending.push_back(path);
                self.focus = true;
            }
        }
    }

    pub fn has_requests(&self) -> bool {
        self.focus || self.reopen || !self.pending.is_empty()
    }

    pub fn open_window(&self, runtime_ready: bool) -> bool {
        self.reopen || (runtime_ready && !self.pending.is_empty())
    }

    pub fn take_focus(&mut self) -> bool {
        std::mem::take(&mut self.focus) | std::mem::take(&mut self.reopen)
    }

    pub fn next_path(&self) -> Option<&str> {
        self.pending.front().map(String::as_str)
    }

    pub fn path_dispatched(&mut self) {
        self.pending.pop_front();
    }
}

#[cfg(target_os = "macos")]
pub fn file_url_path(url: &str) -> Option<String> {
    use core_foundation::{
        base::TCFType,
        string::CFString,
        url::{CFURL, CFURLCreateWithString},
    };
    use std::ptr::null;

    // Ignore other application URLs. CFURL performs filesystem percent decoding.
    if !url.starts_with("file://") {
        return None;
    }
    let url = CFString::new(url);
    let decoded = unsafe { CFURLCreateWithString(null(), url.as_concrete_TypeRef(), null()) };
    if decoded.is_null() {
        return None;
    }
    let decoded = unsafe { CFURL::wrap_under_create_rule(decoded) };
    let path = decoded.to_path()?;
    crate::instance_open::absolute_workspace_path(path.to_str()?).ok()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::file_url_path;

    #[test]
    fn decodes_directory_file_urls_without_accepting_other_schemes() {
        assert_eq!(
            file_url_path("file:///fixture/A%20%E4%B8%AD%E6%96%87%20%25/").as_deref(),
            Some("/fixture/A 中文 %")
        );
        assert_eq!(file_url_path("https://example.invalid/A"), None);
    }
}
