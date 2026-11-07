//! Where the daemon speaks.
//!
//! On Linux the journal timestamps and stores, and a `println!` is
//! enough. On Windows there is no journal: if the daemon only wrote to
//! stdout, the GUI there would have no log at all -- and it was precisely
//! the lack of a log that, on the Linux side, made me misdiagnose a 105 s
//! switch.
//!
//! Hence these macros: one line, two destinations. stdout stays for
//! whoever runs the daemon by hand, and `sys::log_sink` persists wherever
//! the platform needs (on Linux, nothing -- the journal already did it).


#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        $crate::sys::log_sink(&line);
    }};
}

#[macro_export]
macro_rules! gripe {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        eprintln!("{line}");
        $crate::sys::log_sink(&line);
    }};
}
