#[macro_export]
macro_rules! err_format {
    ($($tt:tt)*) => {
        FormatError::msg(format!($($tt)*))
    }
}

#[macro_export]
macro_rules! bail_format {
    ($($tt:tt)*) => {
        return Err(err_format!($($tt)*));
    }
}
