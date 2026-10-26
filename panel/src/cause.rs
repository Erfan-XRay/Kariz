//! What a tunnel's last error most likely means, in words a person can act on.
//!
//! The text a side reports ("Operation timed out (os error 110)") says what its system saw,
//! not what happened on the way between the two servers. Most drops between two countries
//! are one of a few things (the path blocked, packets lost, a connection cut by a reset);
//! this names which. It reads the text, so it works whatever version the server runs. The
//! web panel has the same list (`web/src/cause.ts`).

/// The likely reason for a tunnel's error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// No answer at all to a new connection: the address or port is blocked on the way,
    /// or the path between the two servers is down.
    Unreachable,
    /// The other server answered that nothing listens on the port: its side is not running.
    Refused,
    /// The connection opened, but the tunnel's handshake never finished: something on the
    /// way let it in and then held its traffic back.
    Handshake,
    /// A connection that was open stopped carrying packets and was given up on: packets
    /// lost or throttled on the way.
    Silent,
    /// The connection was cut by a reset: the other side restarted, or something on the way
    /// (often a firewall) cut it.
    Reset,
    /// The other side closed the connection itself (it was stopped or restarted).
    Closed,
    /// Connected, but a test through the tunnel got no answer in time.
    NoAnswer,
    /// The two sides do not have the same token.
    Auth,
}

impl Cause {
    pub fn of(error: &str) -> Option<Self> {
        let e = error.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| e.contains(w));
        // The more particular texts first: "connect to … timed out" and "handshake timed
        // out" are timeouts too, but each says more than one.
        if has(&["authentication failed"]) {
            Some(Self::Auth)
        } else if has(&["no answer through the tunnel", "did not answer in time"]) {
            Some(Self::NoAnswer)
        } else if e.starts_with("connect to ") && e.contains("timed out")
            || has(&[
                "no route to host",
                "network is unreachable",
                "os error 113",
                "os error 101",
            ])
        {
            Some(Self::Unreachable)
        } else if has(&["handshake timed out"]) {
            Some(Self::Handshake)
        } else if has(&["connection refused", "os error 111"]) {
            Some(Self::Refused)
        } else if has(&[
            "connection reset",
            "os error 104",
            "broken pipe",
            "os error 32",
        ]) {
            Some(Self::Reset)
        } else if has(&[
            "os error 110",
            "operation timed out",
            "stopped answering pings",
        ]) {
            Some(Self::Silent)
        } else if has(&["peer closed", "unexpected end of file", "early eof"]) {
            Some(Self::Closed)
        } else {
            None
        }
    }

    /// What it likely means, and what to try.
    pub fn hint(self, fa: bool) -> &'static str {
        match (self, fa) {
            (Self::Unreachable, false) => "No answer at all from the other server: its address or port is blocked on the way, or the path between the two servers is down. If it lasts, try another port or transport (Benchmark).",
            (Self::Unreachable, true) => "سرور مقابل اصلاً جواب نداد: آدرس یا پورتش در مسیر بسته شده یا مسیر بین دو سرور قطع است. اگر ادامه داشت پورت یا ترنسپورت دیگری را امتحان کنید (محک).",
            (Self::Refused, false) => "The other server is reachable but nothing listens on that port: its side of the tunnel is not running.",
            (Self::Refused, true) => "سرور مقابل در دسترس است ولی روی آن پورت چیزی گوش نمی‌دهد: سمت دیگر تانل اجرا نیست.",
            (Self::Handshake, false) => "The connection opened but the tunnel could not finish its handshake: something on the way lets it in and then holds its traffic back (filtering). Another transport may get through (Benchmark).",
            (Self::Handshake, true) => "اتصال باز شد ولی دست‌دادن تانل تمام نشد: چیزی در مسیر اتصال را راه می‌دهد و بعد ترافیکش را نگه می‌دارد (فیلترینگ). ترنسپورت دیگری شاید رد شود (محک).",
            (Self::Silent, false) => "An open connection stopped carrying packets and was given up on: packets are lost or throttled on the way between the two servers.",
            (Self::Silent, true) => "اتصالِ باز دیگر بسته‌ای رد نکرد و رها شد: بسته‌ها در مسیر بین دو سرور گم یا محدود می‌شوند.",
            (Self::Reset, false) => "The connection was cut by a reset: the other side restarted, or something on the way (often a firewall) cut it. If it keeps happening, try another transport (Benchmark).",
            (Self::Reset, true) => "اتصال با یک ریست بریده شد: یا سمت دیگر ری‌استارت شد، یا چیزی در مسیر (اغلب فایروال) آن را برید. اگر تکرار شد ترنسپورت دیگری را امتحان کنید (محک).",
            (Self::Closed, false) => "The other side closed the connection itself: it was stopped or restarted.",
            (Self::Closed, true) => "سمت دیگر خودش اتصال را بست: متوقف یا ری‌استارت شد.",
            (Self::NoAnswer, false) => "Connected, but a test through the tunnel got no answer in time: the link stalled, or the exit side is slow to reach the target.",
            (Self::NoAnswer, true) => "وصل بود ولی آزمایشی که از داخل تانل رد شد به‌موقع جواب نگرفت: لینک گیر کرد، یا سمت خروج دیر به مقصد می‌رسد.",
            (Self::Auth, false) => "The two sides do not have the same token: edit the tunnel and save it again.",
            (Self::Auth, true) => "دو سمت توکن یکسانی ندارند: تانل را ویرایش و دوباره ذخیره کنید.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_errors_people_send_in_have_a_reason() {
        let cases = [
            ("connect to 203.0.113.7:443 timed out", Cause::Unreachable),
            ("Operation timed out (os error 110)", Cause::Silent),
            ("no answer through the tunnel", Cause::NoAnswer),
            (
                "mux session closed: Connection reset by peer (os error 104)",
                Cause::Reset,
            ),
            (
                "mux session closed: mux peer stopped answering pings",
                Cause::Silent,
            ),
            ("TLS or websocket handshake timed out", Cause::Handshake),
            ("Connection refused (os error 111)", Cause::Refused),
            ("No route to host (os error 113)", Cause::Unreachable),
            (
                "mux session closed: peer closed the mux session",
                Cause::Closed,
            ),
            ("authentication failed", Cause::Auth),
        ];
        for (text, cause) in cases {
            assert_eq!(Cause::of(text), Some(cause), "{text}");
        }
        assert_eq!(Cause::of("something new"), None);
    }
}
