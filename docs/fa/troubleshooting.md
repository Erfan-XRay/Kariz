# عیب‌یابی

[English](../troubleshooting.md) · **فارسی**

روی هر دو سمت با `kariz check -c /etc/kariz/config.toml` شروع کنید: فایل را بررسی می‌کند، مقدارهای مؤثر را چاپ می‌کند و هشدارها را می‌گوید. روی تانلِ در حال اجرا، `kariz status -c ...` می‌گوید سمت دیگر وصل هست یا نه و اگر نه، آخرین خطا چه بوده ([Status](status.md)). بعد لاگ‌ها را بخوانید:

```bash
journalctl -u kariz -f                      # with systemd
journalctl -u kariz -p warning              # warnings and errors only
RUST_LOG=kariz=debug kariz run -c ...       # more detail
```

اگر پنل وب دارید، صفحهٔ **لاگ** لاگ هر دو سمت یک تانل را با ترتیب زمانی کنار هم نشان می‌دهد ([پنل](panel.md)).

## دو سمت وصل نمی‌شوند

| پیام لاگ | علت | راه‌حل |
|---|---|---|
| `could not connect to the entry side` / `could not connect to the exit side` | سمتی که گوش می‌دهد در دسترس نیست | `listen` / `remote`، فایروال و security group ابری را بررسی کنید (برای `quic` و `kcp` UDP) |
| `tunnel handshake failed: invalid hello tag` (سمت شنونده)، `rejected the tunnel connection` / `entry side rejected us or failed authentication` (سمت وصل‌شونده) | tokenها فرق دارند | یک token را روی هر دو سمت بگذارید |
| `hello timestamp out of range` | ساعت سرورها چند دقیقه اختلاف دارد | NTP را روشن کنید (`timedatectl set-ntp true`) |
| `peer has mux on but this side has it off` | mux فرق دارد | `[tunnel.mux] enabled` را روی هر دو سمت یکسان کنید |
| `peer uses encryption ..., which tunnel.encryption on this side does not allow` | یک سمت `encryption = "none"` دارد | روی هر دو یا روی هیچ‌کدام بگذارید |
| `404` از شنوندهٔ `ws` / `wss` | `ws.path` یا `ws.host` غلط است | `ws.path` را یکی کنید؛ `ws.host` را روی شنونده ببینید |
| `bad_certificate` (`quic`) | tokenها فرق دارند | یک token را روی هر دو سمت بگذارید |

## وصل می‌شود، ولی

**کاربرها وصل می‌شوند و فوراً قطع می‌شوند.** exit نتوانسته به مقصد برسد. exit `could not connect to target` لاگ می‌کند. `target` از روی exit وصل می‌شود، پس `127.0.0.1:443` پورت ۴۴۳ خودِ exit است.

**UDP کار نمی‌کند.** ببینید قانون `protocol = "udp"` یا `"tcp+udp"` دارد و mux روشن است (`kariz check` غیر از این هشدار می‌دهد). exit نسخهٔ 0.2 اصلاً UDP را پشتیبانی نمی‌کند. قانونی که `duplicate` دارد exit نسخهٔ 0.5 یا جدیدتر می‌خواهد.

**کند است.**

- **روی مسیر پرافت** ترنسپورت‌های مبتنی بر TCP فرو می‌ریزند؛ `kcp` یا `quic` با `congestion = "bbr"` را امتحان کنید.
- **روی مسیر تمیز** یک stream در هر رفت‌وبرگشت به یک window محدود است: `profile = "ultraspeed"` بگذارید یا `tunnel.mux.stream_window` را بالا ببرید.
- [Performance](performance.md#تنظیم-برای-سرعت) را ببینید (انگلیسی).

**بازی زیر بار لگ می‌کند.** روی هر دو سمت `profile = "gaming"` و `kcp` بگذارید. ترنسپورت‌های مبتنی بر TCP بستهٔ بازی را منتظر هر segmentِ گم‌شده نگه می‌دارند. [UDP و بازی](udp-and-games.md) را ببینید.

**پشت CDN، اتصال‌ها بعد از حدود ۱۰۰ ثانیه بی‌کاری قطع می‌شوند.** `tunnel.mux.ping_interval_secs` (یا `tuning.keepalive_secs`) را ۹۰ یا کمتر نگه دارید.

**`quic` یا `kcp` هرگز وصل نمی‌شود.** ممکن است UDP روی مسیر بسته یا محدود شده باشد، به‌خصوص UDP روی پورت ۴۴۳. پورت دیگری امتحان کنید یا به `tcpmux` یا `wss` برگردید.

## هشدارهای `kariz check`

| هشدار | معنی |
|---|---|
| `tunnel.encryption = "none"` | ترافیک روی سیم خواناست |
| ping interval بالای ۹۰ ثانیه روی `ws` / `wss` | CDN اتصال‌های بی‌کار را می‌بُرد |
| UDP بدون mux فوروارد شده | هر جریان UDP یک اتصال کامل تانل می‌گیرد |
| `forward.duplicate has no effect here` | کپی‌ها فقط روی `kcp` (با mux) و `quic` فرستاده می‌شوند |
| `quic.congestion = "bbr"` | در quinn آزمایشی است؛ صف‌ها را پر می‌کند |
| `tls.insecure = true` | گواهی سرور بررسی نمی‌شود |
| `tuning.dscp` روی QUIC یا ویندوز | علامت به سیم نمی‌رسد |
