pub(crate) fn local_date_time(timestamp: u64) -> (String, String) {
    let (_, date, time) = local_date_time_with_year(timestamp);
    (date, time)
}

pub(crate) fn local_date_time_with_year(timestamp: u64) -> (Option<i32>, String, String) {
    // Use the system's local zone, including its offset at the event time.
    #[repr(C)]
    #[derive(Default)]
    struct LocalTime {
        second: i32,
        minute: i32,
        hour: i32,
        day: i32,
        month: i32,
        year: i32,
        weekday: i32,
        year_day: i32,
        is_dst: i32,
        #[cfg(target_os = "macos")]
        utc_offset: std::ffi::c_long,
        #[cfg(target_os = "macos")]
        timezone: *const std::ffi::c_char,
    }
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn localtime_r(seconds: *const i64, output: *mut LocalTime) -> *mut LocalTime;
    }
    #[cfg(target_os = "windows")]
    unsafe extern "C" {
        fn _localtime64_s(output: *mut LocalTime, seconds: *const i64) -> i32;
    }
    let seconds = (timestamp / 1_000) as i64;
    let mut local = LocalTime::default();
    // Each function fills the caller-owned C tm for this platform.
    #[cfg(target_os = "macos")]
    let valid = unsafe { !localtime_r(&seconds, &mut local).is_null() };
    #[cfg(target_os = "windows")]
    let valid = unsafe { _localtime64_s(&mut local, &seconds) == 0 };
    if valid {
        (
            Some(local.year + 1900),
            format!("{:02}/{:02}", local.month + 1, local.day),
            format!("{:02}:{:02}", local.hour, local.minute),
        )
    } else {
        (
            None,
            "—".into(),
            format!("{:02}:{:02}", seconds / 3_600 % 24, seconds / 60 % 60),
        )
    }
}
