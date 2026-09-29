//! How `kariz speedtest` shows its result.

use kariz::config::{mode_name, role_name, Config};
use kariz::speedtest::{Latency, Rate, Report};

use crate::logging::{Style, AQUA, BOLD, DIM, RED, SAND, TEAL, YELLOW};

/// "612.3 Mbit/s", or "1.24 Gbit/s" from a thousand up.
fn speed(mbps: f64) -> String {
    if mbps >= 1000.0 {
        format!("{:.2} Gbit/s", mbps / 1000.0)
    } else {
        format!("{mbps:.1} Mbit/s")
    }
}

fn millis(ms: f64) -> String {
    format!("{ms:.1} ms")
}

/// Green up to 1 %, yellow up to 5 %, red above.
fn loss_color(percent: f64) -> &'static str {
    if percent < 1.0 {
        TEAL
    } else if percent < 5.0 {
        YELLOW
    } else {
        RED
    }
}

fn rate_line(style: &Style, arrow: &str, name: &str, rate: &Rate) -> String {
    format!(
        "  {} {}   {}   {}",
        style.paint(TEAL, arrow),
        style.paint(BOLD, &format!("{name:<9}")),
        style.paint(
            &format!("{BOLD}{AQUA}"),
            &format!("{:>13}", speed(rate.mbps))
        ),
        style.paint(DIM, &format!("best second {}", speed(rate.peak_mbps))),
    )
}

fn latency_row(style: &Style, label: &str, l: &Latency) -> String {
    let cells = if l.received == 0 {
        style.paint(RED, "no answers")
    } else {
        format!(
            "{}  {}  {}",
            style.paint(AQUA, &format!("{:>9}", millis(l.p50_ms))),
            style.paint(AQUA, &format!("{:>9}", millis(l.p99_ms))),
            style.paint(AQUA, &format!("{:>9}", millis(l.jitter_ms))),
        )
    };
    let lost = l.loss_percent();
    let loss = if lost > 0.0 {
        format!(
            "   {}",
            style.paint(loss_color(lost), &format!("{lost:.1} % lost"))
        )
    } else {
        String::new()
    };
    format!(
        "    {}  {cells}{loss}",
        style.paint(DIM, &format!("{label:<15}"))
    )
}

/// The whole report, ready to print.
pub fn render(style: &Style, config: &Config, r: &Report) -> String {
    let mut out = vec![String::new()];
    out.push(format!(
        "  {} {} · profile {}   {}",
        style.paint(TEAL, "▸"),
        style.paint(BOLD, config.tunnel.transport.name()),
        config.profile.name(),
        style.paint(
            DIM,
            &format!(
                "{} · {} mode",
                role_name(config.role),
                mode_name(config.mode)
            )
        ),
    ));
    out.push(format!(
        "  {} {} streams × {} s each way",
        style.paint(TEAL, "▸"),
        r.streams,
        r.seconds
    ));
    out.push(String::new());
    out.push(rate_line(style, "↓", "download", &r.download));
    out.push(rate_line(style, "↑", "upload", &r.upload));
    out.push(String::new());
    out.push(format!(
        "  {} {}",
        style.paint(TEAL, "◆"),
        style.paint(
            BOLD,
            &format!(
                "latency         {:>9}  {:>9}  {:>9}",
                "median", "p99", "jitter"
            )
        )
    ));
    out.push(latency_row(style, "idle", &r.idle));
    out.push(latency_row(style, "during download", &r.download_latency));
    out.push(latency_row(style, "during upload", &r.upload_latency));
    if let Some(udp) = &r.udp {
        out.push(String::new());
        let lost = udp.loss_percent();
        out.push(format!(
            "  {} {}   {} {}   {}",
            style.paint(TEAL, "✦"),
            style.paint(BOLD, "UDP"),
            style.paint(
                &format!("{BOLD}{}", loss_color(lost)),
                &format!("{lost:.1} % lost")
            ),
            style.paint(
                DIM,
                &format!(
                    "({} of {})",
                    udp.sent - udp.received.min(udp.sent),
                    udp.sent
                )
            ),
            if udp.received == 0 {
                style.paint(RED, "no answers")
            } else {
                format!(
                    "median {} · p99 {} · jitter {}",
                    style.paint(AQUA, &millis(udp.p50_ms)),
                    style.paint(AQUA, &millis(udp.p99_ms)),
                    style.paint(AQUA, &millis(udp.jitter_ms)),
                )
            }
        ));
    }
    for note in &r.notes {
        out.push(format!(
            "  {} {}",
            style.paint(SAND, "•"),
            style.paint(DIM, note)
        ));
    }
    out.push(String::new());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::UtcOffset;

    fn plain() -> Style {
        Style {
            color: false,
            journal: false,
            offset: UtcOffset::UTC,
        }
    }

    fn config() -> Config {
        Config::parse(
            "role = \"entry\"\nmode = \"direct\"\nprofile = \"ultraspeed\"\n\
             [tunnel]\ntransport = \"tcpmux\"\nremote = \"203.0.113.1:3080\"\n\
             token = \"test-token-0123456789\"\n\
             [[forward]]\nlisten = \"0.0.0.0:443\"\ntarget = \"127.0.0.1:443\"\n",
        )
        .unwrap()
    }

    fn latency(p50: f64, sent: u32, received: u32) -> Latency {
        Latency {
            sent,
            received,
            p50_ms: p50,
            p99_ms: p50 * 2.0,
            jitter_ms: 1.5,
        }
    }

    #[test]
    fn the_report_shows_speed_latency_and_udp() {
        let report = Report {
            seconds: 10,
            streams: 4,
            idle: latency(61.2, 20, 20),
            download: Rate {
                mbps: 1240.0,
                peak_mbps: 1300.5,
                bytes: 1,
            },
            download_latency: latency(79.0, 100, 99),
            upload: Rate {
                mbps: 587.1,
                peak_mbps: 640.2,
                bytes: 1,
            },
            upload_latency: latency(0.0, 100, 0),
            udp: Some(latency(62.0, 320, 318)),
            notes: vec!["a note".into()],
        };
        let text = render(&plain(), &config(), &report);
        assert!(text.contains("tcpmux · profile ultraspeed"), "{text}");
        assert!(text.contains("entry · direct mode"));
        assert!(text.contains("4 streams × 10 s"));
        assert!(
            text.contains("1.24 Gbit/s") && text.contains("best second 1.30 Gbit/s"),
            "{text}"
        );
        assert!(text.contains("587.1 Mbit/s"));
        assert!(text.contains("61.2 ms"));
        assert!(text.contains("1.0 % lost"), "{text}");
        assert!(text.contains("no answers"));
        assert!(text.contains("0.6 % lost (2 of 320)"), "{text}");
        assert!(text.contains("a note"));
        assert!(!text.contains('\x1b'));
        let mut no_udp = report.clone();
        no_udp.udp = None;
        assert!(!render(&plain(), &config(), &no_udp).contains("UDP"));
    }

    #[test]
    fn loss_gets_a_colour_by_size() {
        assert_eq!(loss_color(0.0), TEAL);
        assert_eq!(loss_color(2.0), YELLOW);
        assert_eq!(loss_color(9.0), RED);
    }
}
