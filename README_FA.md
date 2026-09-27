<div dir="rtl">

# کاریز

[English](README.md) | **فارسی**

کاریز یک هسته‌ی تانل برای اتصال سرورها به هم است که با Rust نوشته شده و سه هدف اصلی دارد:

1. **مصرف کم منابع**: بدون Garbage Collector، حدود ۵ مگابایت RAM برای هر سمت، مناسب VPSهای ارزان.
2. **عبور از DPI**: ترافیک رمزشده بدون هیچ بایت ثابت، WebSocket شبیه مرورگر، و کار کردن از پشت CDN.
3. **بیشترین سرعت**: مسیر داده بدون dispatch پویا، سوکت‌های تنظیم‌شده و پروفایل‌های مخصوص هر کاربرد.

> وضعیت: **نسخه‌ی 0.2.0.** فرمت داده‌ها روی شبکه بعد از 0.1.0 عوض شده؛ هر دو سرور را با هم آپدیت کنید.
> کارهای بعدی (UDP، KCP، QUIC، ICMP و پروفایل استتار) در [docs/ROADMAP.md](docs/ROADMAP.md) و فهرست تغییرات در [CHANGELOG.md](CHANGELOG.md) است.

## امکانات

- **transportها:**

  | transport | چه چیزی روی شبکه می‌رود | کِی استفاده کنیم |
  |---|---|---|
  | `tcp` | یک اتصال TCP برای هر اتصال کاربر | راه‌اندازی ساده، بیشترین سرعت روی یک اتصال |
  | `tcpmux` | چند اتصال TCP بلندمدت که همه‌ی اتصال‌های کاربر از داخلشان رد می‌شوند | اتصال‌های کاربر زیاد؛ اتصال کمتر بین دو سرور |
  | `ws` | WebSocket روی TCP (HTTP) | پشت CDN یا reverse proxy که با سرور مبدأ HTTP حرف می‌زند |
  | `wss` | WebSocket روی TLS (HTTPS) | پشت CDN، یا بدون CDN برای اینکه شبیه یک سایت HTTPS باشد |

- **رمزنگاری** (`tunnel.encryption`، پیش‌فرض `auto`):
  - دو سرور با یک توکن مشترک همدیگر را احراز هویت می‌کنند و خود توکن هیچ‌وقت روی شبکه نمی‌رود.
  - تبادل کلید X25519 امنیت پیشرو (forward secrecy) می‌دهد.
  - داده‌ها در بسته‌های AES-256-GCM یا ChaCha20-Poly1305 فرستاده می‌شوند.
  - پیام شروع (hello) نه بایت ثابت دارد نه طول ثابت، و hello تکراری (replay) رد می‌شود.
  - اتصالی که handshake را رد کند فوراً بسته نمی‌شود، پس probe از زمان بسته‌شدن چیزی نمی‌فهمد.
- **mux** (`[tunnel.mux]`):
  - اتصال‌های کاربر به‌صورت stream روی چند اتصال بلندمدت می‌روند.
  - هر stream کنترل جریان خودش را دارد، پس stream کُند بقیه را معطل نمی‌کند.
  - باز کردن stream رفت‌وبرگشت اضافه ندارد.
  - pingها اتصال مرده را تشخیص می‌دهند و نمی‌گذارند CDN اتصال بیکار را قطع کند.
  - اتصال‌ها را می‌شود به‌صورت دوره‌ای عوض کرد.
- **WebSocket:**
  - درخواست upgrade شبیه مرورگر است.
  - با early data، handshake داخل خود درخواست upgrade می‌رود و پشت CDN یک رفت‌وبرگشت صرفه‌جویی می‌شود.
  - هر درخواستی که upgrade معتبر روی مسیر تنظیم‌شده نباشد، همان صفحه‌ی 404 پیش‌فرض nginx را می‌گیرد.
- **TLS** (`wss`):
  - با rustls پیاده شده.
  - سمت dial گواهی سرور را با root certificateهای Mozilla بررسی می‌کند، یا فقط یک گواهی self-signed مشخص را می‌پذیرد (`kariz pin`).
  - سمت listen با تغییر فایل‌های گواهی آن را دوباره بارگذاری می‌کند، پس تمدید Let's Encrypt ری‌استارت لازم ندارد.
- **حالت‌های reverse و direct** برای همه‌ی transportها، با اتصال دوباره‌ی خودکار.

## مفاهیم

| اصطلاح | معنی |
|---|---|
| **entry** | سروری که کاربرها به آن وصل می‌شوند (مثلاً سرور ایران). قوانین `[[forward]]` روی این سمت تعریف می‌شوند. |
| **exit** | سروری که به مقصد واقعی وصل می‌شود (مثلاً سرور خارج). |
| حالت **reverse** | سمت exit به entry وصل می‌شود و اتصال‌ها را آماده نگه می‌دارد. |
| حالت **direct** | سمت entry به exit وصل می‌شود. |

سمتی که وصل می‌شود (dial می‌کند) به `tunnel.remote` نیاز دارد و سمت دیگر به `tunnel.listen`.

## شروع سریع

فایل اجرایی لینوکس (x86_64، بدون وابستگی) را از بخش [Releases](https://github.com/Erfan-XRay/Kariz/releases) دانلود کنید، یا خودتان build کنید:

<div dir="ltr">

```bash
cargo build --release
sudo cp target/release/kariz /usr/local/bin/
kariz token                               # generate a shared token
```

</div>

یک جفت از نمونه کانفیگ‌ها را انتخاب کنید، توکن را در هر دو بگذارید و آدرس‌ها را تنظیم کنید:

| نمونه‌ها (پوشه‌ی `configs/`) | کاربرد |
|---|---|
| `entry-reverse.toml` و `exit-reverse.toml` | حالت reverse روی `tcp`: سرور exit به entry وصل می‌شود |
| `entry-direct.toml` و `exit-direct.toml` | حالت direct روی `tcp`: سرور entry به exit وصل می‌شود |
| `entry-tcpmux-reverse.toml` و `exit-tcpmux-reverse.toml` | حالت reverse با mux |
| `entry-wss-direct.toml` و `exit-wss-direct.toml` | `wss` مستقیم به سرور خودتان، با گواهی self-signed که pin شده |
| `entry-wss-cdn.toml` و `exit-wss-cdn.toml` | `wss` از پشت CDN مثل Cloudflare (راهنما: [docs/CDN.md](docs/CDN.md)) |

<div dir="ltr">

```bash
kariz check -c /etc/kariz/config.toml     # validate, print a summary and warnings
kariz run -c /etc/kariz/config.toml       # run (or use systemd, below)
```

</div>

فایل سرویس systemd در [`systemd/kariz.service`](systemd/kariz.service) است و کانفیگ را از `/etc/kariz/config.toml` می‌خواند:

<div dir="ltr">

```bash
sudo cp systemd/kariz.service /etc/systemd/system/
sudo systemctl enable --now kariz
journalctl -u kariz -f
```

</div>

## دستورها

| دستور | کار |
|---|---|
| `kariz run -c <file>` | اجرای یک سمت تانل |
| `kariz check -c <file>` | بررسی کانفیگ و چاپ خلاصه و هشدارها |
| `kariz token` | ساخت توکن تصادفی برای `tunnel.token` |
| `kariz pin <cert.pem>` | چاپ مقدار `tunnel.tls.pin_sha256` یک گواهی |

برای لاگ کامل‌تر `RUST_LOG=kariz=debug` را تنظیم کنید؛ این مقدار بر `[log] level` اولویت دارد.

## کانفیگ

همه‌ی جدول‌ها به‌جز `[tunnel]` اختیاری‌اند و هر فیلد در نمونه کانفیگ‌ها هم با توضیح آمده است. کانفیگ‌های نسخه‌ی 0.1 همچنان کار می‌کنند. توضیح فیلدها در [README انگلیسی](README.md#configuration) است؛ خلاصه‌ی مهم‌ترین‌ها:

| فیلد | توضیح |
|---|---|
| `tunnel.transport` | `tcp`، `tcpmux`، `ws` یا `wss` |
| `tunnel.remote` / `tunnel.listen` | آدرس سمت مقابل (یا لبه‌ی CDN) برای سمت dial، و آدرس گوش دادن برای سمت دیگر |
| `tunnel.token` | در هر دو سمت یکی و دست‌کم ۱۶ کاراکتر |
| `tunnel.encryption` | `auto`، `chacha20-poly1305`، `aes-256-gcm` یا `none` (در هر دو سمت) |
| `[tunnel.mux]` | باید در هر دو سمت روشن یا خاموش باشد؛ پیش‌فرض برای `tcpmux`، `ws` و `wss` روشن است |
| `tunnel.ws.path` | مسیر WebSocket، در هر دو سمت یکی |
| `tunnel.ws.host` | در سمت dial، هدر Host (و SNI)؛ در سمت listen، درخواست‌های hostهای دیگر رد می‌شوند |
| `tunnel.ws.early_data` | در سمت dial: hello داخل درخواست upgrade |
| `tunnel.tls.pin_sha256` | در سمت dial: فقط همین گواهی پذیرفته می‌شود (با `kariz pin`) |
| `tunnel.tls.cert` / `key` | در سمت listen: گواهی و کلید؛ با تغییر فایل‌ها دوباره بارگذاری می‌شوند |
| `tuning.keepalive_secs` | فاصله‌ی keepalive و ping در mux؛ پشت CDN حداکثر ۹۰ |

### پروفایل‌ها

| | `balanced` (پیش‌فرض) | `throughput` | `gaming` |
|---|---|---|---|
| بافر relay | 64 KiB | 256 KiB | 16 KiB |
| keepalive / ping در mux | 30 ثانیه | 30 ثانیه | 10 ثانیه |
| پنجره‌ی هر stream در mux | 256 KiB | 1 MiB | 64 KiB |
| تعداد اتصال‌های mux | 4 | 8 | 2 |
| یکی کردن نوشتن‌ها در mux | روشن | روشن | خاموش |

هر مقداری را می‌توان در بخش `[tuning]` تغییر داد.

## کارایی

اندازه‌گیری روی localhost: کاربر ← entry ← exit ← سرور echo، با ۲۵۶ مگابایت داده. هر دو سمت و سرور echo روی یک ماشین مجازی ۴ هسته‌ای Xeon با فرکانس 2.1 GHz اجرا شدند و CPU مشترک بود. پس این اعداد برای مقایسه‌ی حالت‌ها با هم است؛ در لینک واقعی بین دو سرور معمولاً اول شبکه محدودیت ایجاد می‌کند. جدول سرعت و حافظه در [README انگلیسی](README.md#performance) است. به‌طور خلاصه:

- `tcp` با AES: حدود 3.8 تا 4.0 گیگابیت بر ثانیه
- `ws` با mux: حدود 2.8 تا 3.1 گیگابیت بر ثانیه
- `wss` با mux: حدود 2.3 تا 2.5 گیگابیت بر ثانیه
- مصرف حافظه در حالت بیکار: حدود ۵ تا ۶ مگابایت برای هر سمت
- ۱۰۰ اتصال بیکار روی mux: کمتر از ۱ مگابایت حافظه‌ی اضافه

## نکته‌های امنیتی

- توکن تنها راز است و هر کسی آن را داشته باشد می‌تواند از تانل استفاده کند. آن را با `kariz token` بسازید و جایی به اشتراک نگذارید.
- `encryption = "none"` فقط احراز هویت می‌کند و ترافیک روی شبکه خواناست. فقط داخل یک لایه‌ی رمزشده‌ی دیگر از آن استفاده کنید.
- پشت CDN، اتصال TLS در خود CDN تمام می‌شود، اما رمزنگاری خود تانل همچنان محتوا را از CDN هم پنهان نگه می‌دارد.
- ClientHello کتابخانه‌ی rustls شبیه مرورگر نیست، و درخواست HTTP ساده به پورت `wss` به‌جای صفحه‌ی nginx خطای TLS می‌گیرد. هر دو مورد برای فاز ۶ (استتار) برنامه‌ریزی شده‌اند.

## توسعه

<div dir="ltr">

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test                          # nginx tests run too when nginx is installed
cargo test --release --test tunnel throughput -- --ignored --nocapture
scripts/rss.sh tcpmux 100           # memory, after cargo build --release
```

</div>

مستندات طراحی: [docs/ROADMAP.md](docs/ROADMAP.md) و [docs/PHASE2.md](docs/PHASE2.md).

</div>
