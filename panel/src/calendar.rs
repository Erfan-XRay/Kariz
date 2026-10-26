//! Days, months and the periods of a monthly quota, in the Persian (Jalali) or the
//! Gregorian calendar, at a fixed offset from UTC (the panel's time zone; Iran has had no
//! daylight saving since 2022).
//!
//! The Jalali conversion is the arithmetic of jalaali-js (Borkowski's leap years with the
//! break years), right from 1178 to 3177 AP. The browser has the same in `calendar.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Calendar {
    #[default]
    Jalali,
    Gregorian,
}

impl Calendar {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "jalali" => Some(Self::Jalali),
            "gregorian" => Some(Self::Gregorian),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Jalali => "jalali",
            Self::Gregorian => "gregorian",
        }
    }

    /// The date of day `days` (since 1970-01-01).
    pub fn date(self, days: i64) -> (i64, u32, u32) {
        let jdn = days + UNIX_JDN;
        match self {
            Self::Gregorian => d2g(jdn),
            Self::Jalali => d2j(jdn),
        }
    }

    /// The day (since 1970-01-01) of a date.
    pub fn day(self, y: i64, m: u32, d: u32) -> i64 {
        let jdn = match self {
            Self::Gregorian => g2d(y, i64::from(m), i64::from(d)),
            Self::Jalali => j2d(y, i64::from(m), i64::from(d)),
        };
        jdn - UNIX_JDN
    }

    pub fn month_len(self, y: i64, m: u32) -> u32 {
        match self {
            Self::Gregorian => match m {
                2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            },
            Self::Jalali => match m {
                1..=6 => 31,
                7..=11 => 30,
                _ if jal_cal(y).0 == 0 => 30,
                _ => 29,
            },
        }
    }

    /// The month after `(y, m)`.
    fn next_month(y: i64, m: u32) -> (i64, u32) {
        if m == 12 {
            (y + 1, 1)
        } else {
            (y, m + 1)
        }
    }

    fn prev_month(y: i64, m: u32) -> (i64, u32) {
        if m == 1 {
            (y - 1, 12)
        } else {
            (y, m - 1)
        }
    }

    /// The day `reset_day` of a month, or its last day in a shorter month.
    fn reset_in(self, y: i64, m: u32, reset_day: u32) -> i64 {
        self.day(y, m, reset_day.clamp(1, self.month_len(y, m)))
    }

    /// The period of a monthly quota that renews on day `reset_day` and holds the moment
    /// `at`: its start and the start of the next, in unix seconds, at local midnight.
    pub fn period(self, offset_min: i64, reset_day: u32, at: i64) -> (i64, i64) {
        let off = offset_min * 60;
        let today = (at + off).div_euclid(86_400);
        let (y, m, _) = self.date(today);
        let this = self.reset_in(y, m, reset_day);
        let (start, end) = if today >= this {
            let (ny, nm) = Self::next_month(y, m);
            (this, self.reset_in(ny, nm, reset_day))
        } else {
            let (py, pm) = Self::prev_month(y, m);
            (self.reset_in(py, pm, reset_day), this)
        };
        (start * 86_400 - off, end * 86_400 - off)
    }
}

/// The Julian day number of 1970-01-01.
const UNIX_JDN: i64 = 2_440_588;

const BREAKS: [i64; 20] = [
    -61, 9, 38, 199, 426, 686, 756, 818, 1111, 1181, 1210, 1635, 2060, 2097, 2192, 2262, 2324,
    2394, 2456, 3178,
];

/// For Jalali year `jy`: where it is in the leap cycle (0: a leap year), the Gregorian
/// year it starts in, and the day of March its first day falls on.
fn jal_cal(jy: i64) -> (i64, i64, i64) {
    let gy = jy + 621;
    let mut leap_j = -14;
    let mut jp = BREAKS[0];
    let mut jump = 0;
    for &jm in &BREAKS[1..] {
        jump = jm - jp;
        if jy < jm {
            break;
        }
        leap_j += jump / 33 * 8 + (jump % 33) / 4;
        jp = jm;
    }
    let mut n = jy - jp;
    leap_j += n / 33 * 8 + ((n % 33) + 3) / 4;
    if jump % 33 == 4 && jump - n == 4 {
        leap_j += 1;
    }
    let leap_g = gy / 4 - (gy / 100 + 1) * 3 / 4 - 150;
    let march = 20 + leap_j - leap_g;
    if jump - n < 6 {
        n = n - jump + (jump + 4) / 33 * 33;
    }
    let mut leap = (((n + 1) % 33) - 1) % 4;
    if leap == -1 {
        leap = 4;
    }
    (leap, gy, march)
}

fn j2d(jy: i64, jm: i64, jd: i64) -> i64 {
    let (_, gy, march) = jal_cal(jy);
    g2d(gy, 3, march) + (jm - 1) * 31 - jm / 7 * (jm - 7) + jd - 1
}

fn d2j(jdn: i64) -> (i64, u32, u32) {
    let (gy, _, _) = d2g(jdn);
    let mut jy = gy - 621;
    let (leap, _, march) = jal_cal(jy);
    let mut k = jdn - g2d(gy, 3, march);
    if k >= 0 {
        if k <= 185 {
            return (jy, (1 + k / 31) as u32, (k % 31 + 1) as u32);
        }
        k -= 186;
    } else {
        jy -= 1;
        k += 179;
        if leap == 1 {
            k += 1;
        }
    }
    (jy, (7 + k / 30) as u32, (k % 30 + 1) as u32)
}

fn g2d(gy: i64, gm: i64, gd: i64) -> i64 {
    let d = (gy + (gm - 8) / 6 + 100_100) * 1461 / 4 + (153 * ((gm + 9) % 12) + 2) / 5 + gd
        - 34_840_408;
    d - (gy + 100_100 + (gm - 8) / 6) / 100 * 3 / 4 + 752
}

fn d2g(jdn: i64) -> (i64, u32, u32) {
    let mut j = 4 * jdn + 139_361_631;
    j += (4 * jdn + 183_187_720) / 146_097 * 3 / 4 * 4 - 3908;
    let i = (j % 1461) / 4 * 5 + 308;
    let gd = (i % 153) / 5 + 1;
    let gm = (i / 153) % 12 + 1;
    let gy = j / 1461 - 100_100 + (8 - gm) / 6;
    (gy, gm as u32, gd as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Calendar::{Gregorian, Jalali};

    fn g(y: i64, m: u32, d: u32) -> i64 {
        Gregorian.day(y, m, d)
    }

    #[test]
    fn gregorian_days_round_trip() {
        assert_eq!(g(1970, 1, 1), 0);
        assert_eq!(g(2000, 3, 1), 11_017);
        for day in [-1, 0, 59, 11_016, 20_000, 30_000] {
            let (y, m, d) = Gregorian.date(day);
            assert_eq!(g(y, m, d), day);
        }
    }

    #[test]
    fn jalali_dates_are_right() {
        // Nowruz 1405 is 21 March 2026, and today 4 Aban 1405.
        assert_eq!(Jalali.date(g(2026, 3, 21)), (1405, 1, 1));
        assert_eq!(Jalali.date(g(2026, 10, 26)), (1405, 8, 4));
        assert_eq!(Jalali.date(g(2025, 3, 21)), (1404, 1, 1));
        // 1403 is a leap year: Esfand has 30 days, the last 20 March 2025.
        assert_eq!(Jalali.date(g(2025, 3, 20)), (1403, 12, 30));
        assert_eq!(Jalali.month_len(1403, 12), 30);
        assert_eq!(Jalali.month_len(1404, 12), 29);
        assert_eq!(Jalali.date(g(1979, 2, 11)), (1357, 11, 22));
        for day in (0..25_000).step_by(37) {
            let (y, m, d) = Jalali.date(day);
            assert_eq!(Jalali.day(y, m, d), day, "{y}/{m}/{d}");
            assert!(d <= Jalali.month_len(y, m));
        }
    }

    #[test]
    fn a_quota_period_starts_on_its_day_at_local_midnight() {
        let tehran = 210;
        let local = |y, m, d, h: i64| g(y, m, d) * 86_400 + h * 3600 - tehran * 60;
        // 4 Aban, renewing on the 1st: from 1 Aban (23 October) to 1 Azar (22 November).
        let at = local(2026, 10, 26, 12);
        assert_eq!(
            Jalali.period(tehran, 1, at),
            (local(2026, 10, 23, 0), local(2026, 11, 22, 0))
        );
        // Renewing on the 10th: still in the period from 10 Mehr.
        let (start, end) = Jalali.period(tehran, 10, at);
        assert_eq!(Jalali.date(start.div_euclid(86_400) + 1), (1405, 7, 10));
        assert_eq!(end, local(2026, 11, 1, 0));
        // Gregorian, on the 31st: February has none, so its last day stands in.
        let feb = local(2026, 2, 15, 9);
        assert_eq!(
            Gregorian.period(tehran, 31, feb),
            (local(2026, 1, 31, 0), local(2026, 2, 28, 0))
        );
        // Just before local midnight is still the old day.
        let late = local(2026, 10, 22, 23) + 3599;
        assert_eq!(Jalali.period(tehran, 1, late).1, local(2026, 10, 23, 0));
    }
}
