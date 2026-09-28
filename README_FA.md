<div dir="rtl">

# کاریز

[English](README.md) | **فارسی**

کاریز یک هسته‌ی تانل برای اتصال سرورها به هم است که با Rust نوشته شده و سه هدف اصلی دارد:

1. **مصرف کم منابع**: بدون Garbage Collector، حدود ۵ مگابایت RAM برای هر سمت، مناسب VPSهای ارزان.
2. **عبور از DPI**: ترافیک رمزشده بدون هیچ بایت ثابت، WebSocket شبیه مرورگر، و کار کردن از پشت CDN.
3. **بیشترین سرعت**: مسیر داده بدون dispatch پویا، سوکت‌های تنظیم‌شده و پروفایل‌های مخصوص هر کاربرد.

> وضعیت: **نسخه‌ی 0.4.0.** نسخه‌ی 0.4 روی `tcp`، `tcpmux`، `ws` و `wss` با 0.3 کار می‌کند؛ برای `quic` و `kcp` هر دو سمت باید 0.4 باشند.
> کارهای بعدی (ICMP و پروفایل‌های بازی و استتار) در [docs/ROADMAP.md](docs/ROADMAP.md) و فهرست تغییرات در [CHANGELOG.md](CHANGELOG.md) است.

## امکانات

- **transportها:**

  | transport | چه چیزی روی شبکه می‌رود | کِی استفاده کنیم |
  |---|---|---|
  | `tcp` | یک اتصال TCP برای هر اتصال کاربر | راه‌اندازی ساده، بیشترین سرعت روی یک اتصال |
  | `tcpmux` | چند اتصال TCP بلندمدت که همه‌ی اتصال‌های کاربر از داخلشان رد می‌شوند | اتصال‌های کاربر زیاد؛ اتصال کمتر بین دو سرور |
  | `ws` | WebSocket روی TCP (HTTP) | پشت CDN یا reverse proxy که با سرور مبدأ HTTP حرف می‌زند |
  | `wss` | WebSocket روی TLS (HTTPS) | پشت CDN، یا بدون CDN برای اینکه شبیه یک سایت HTTPS باشد |
  | `quic` | QUIC روی UDP: stream و datagram، با TLS 1.3 | flowهای UDP بدون معطل شدن پشت بسته‌ی گم‌شده؛ مسیرهایی که UDP خوب کار می‌کند |
  | `kcp` | KCP روی UDP، هر بسته رمزشده، با FEC اختیاری | لینک‌های پرتلفات که TCP در آن‌ها از سرعت می‌افتد |

  `quic` و `kcp` روی UDP کار می‌کنند و بعضی شبکه‌ها UDP را محدود یا مسدود می‌کنند (به‌خصوص UDP روی پورت 443). این‌ها گزینه‌ای برای مسیرهایی‌اند که UDP در آن‌ها کار می‌کند، نه جایگزین بقیه؛ عوض کردن transport فقط یک خط کانفیگ است.

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
- **forward کردن UDP** (با `protocol = "udp"` یا `"tcp+udp"` در قانون `[[forward]]`):
  - روی همه‌ی transportها کار می‌کند، پس UDP (WireGuard، بازی، DNS، QUIC) جایی که خود UDP بسته است هم کار می‌کند.
  - هر آدرس کلاینت یک flow است و در سمت exit سوکت جدای خودش را دارد.
  - مرز بسته‌ها حفظ می‌شود، بسته‌ها جلوتر از داده‌ی حجیم TCP روی همان اتصال فرستاده می‌شوند، و اگر تانل عقب بماند بسته دور ریخته می‌شود به‌جای اینکه صف بکشد.
  - روی transportهای مبتنی بر TCP، گم شدن یک بسته روی لینک همچنان بسته‌های پشت سرش را معطل می‌کند. روی `quic` هر بسته یک datagram جداست و گم شدنش چیز دیگری را معطل نمی‌کند.
- **QUIC** (`quic`):
  - هر اتصال کاربر یک stream جدای QUIC است و اتصال‌ها همدیگر را معطل نمی‌کنند.
  - دو سمت با کلیدهایی که از توکن ساخته می‌شوند همدیگر را احراز هویت می‌کنند (TLS 1.3 دوطرفه، بدون فایل گواهی).
  - کنترل ازدحام: Cubic (پیش‌فرض)، BBR یا NewReno.
  - handshake شبیه HTTP/3 است (ALPN `h3` و SNI قابل تنظیم).
- **KCP** (`kcp`):
  - بسته‌های گم‌شده را سریع دوباره می‌فرستد و زیر loss سرعتش را حفظ می‌کند؛ presetها از ملایم تا تهاجمی.
  - هر بسته‌ی UDP با کلیدی از توکن رمز می‌شود، پس پورت به هیچ چیز دیگری جواب نمی‌دهد و هدرهای KCP هم پنهان‌اند.
  - FEC اختیاری با Reed-Solomon بسته‌ی گم‌شده را بدون ارسال دوباره بازسازی می‌کند.
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

> **راهنمای کامل راه‌اندازی روی سرور:** [docs/SETUP_FA.md](docs/SETUP_FA.md). این راهنما قدم‌به‌قدم از نصب تا تست سرعت با iperf3، همه‌ی transportها، تنظیمات سیستم و عیب‌یابی را پوشش می‌دهد. این بخش فقط خلاصه است.

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
| `entry-udp-reverse.toml` و `exit-udp-reverse.toml` | forward کردن UDP (سرور WireGuard و یک سرور بازی روی TCP+UDP) با mux |
| `entry-wss-direct.toml` و `exit-wss-direct.toml` | `wss` مستقیم به سرور خودتان، با گواهی self-signed که pin شده |
| `entry-wss-cdn.toml` و `exit-wss-cdn.toml` | `wss` از پشت CDN مثل Cloudflare (راهنما: [docs/CDN.md](docs/CDN.md)) |
| `entry-quic-direct.toml` و `exit-quic-direct.toml` | `quic`، با WireGuard روی datagramهای QUIC |
| `entry-kcp-reverse.toml` و `exit-kcp-reverse.toml` | `kcp` برای لینک پرتلفات (FEC در توضیحات) |

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
| `tunnel.transport` | `tcp`، `tcpmux`، `ws`، `wss`، `quic` یا `kcp` |
| `tunnel.remote` / `tunnel.listen` | آدرس سمت مقابل (یا لبه‌ی CDN) برای سمت dial، و آدرس گوش دادن برای سمت دیگر |
| `tunnel.token` | در هر دو سمت یکی و دست‌کم ۱۶ کاراکتر |
| `tunnel.encryption` | `auto`، `chacha20-poly1305`، `aes-256-gcm` یا `none` (در هر دو سمت؛ برای `quic` همیشه TLS 1.3 است و باید `auto` بماند) |
| `[tunnel.mux]` | باید در هر دو سمت روشن یا خاموش باشد؛ پیش‌فرض برای `tcpmux`، `ws`، `wss` و `kcp` روشن است و برای `quic` همیشه روشن |
| `tunnel.ws.path` | مسیر WebSocket، در هر دو سمت یکی |
| `tunnel.ws.host` | در سمت dial، هدر Host (و SNI)؛ در سمت listen، درخواست‌های hostهای دیگر رد می‌شوند |
| `tunnel.ws.early_data` | در سمت dial: hello داخل درخواست upgrade |
| `tunnel.tls.pin_sha256` | در سمت dial: فقط همین گواهی پذیرفته می‌شود (با `kariz pin`) |
| `tunnel.tls.cert` / `key` | در سمت listen: گواهی و کلید؛ با تغییر فایل‌ها دوباره بارگذاری می‌شوند |
| `tunnel.quic.congestion` | `cubic` (پیش‌فرض)، `bbr` یا `newreno` |
| `tunnel.quic.sni` / `alpn` | نام سرور در handshake (سمت dial؛ پیش‌فرض host سمت مقابل) و ALPN (پیش‌فرض `h3`) |
| `tunnel.kcp.mode` | `normal`، `fast`، `fast2` (پیش‌فرض)، `fast3` یا `manual` |
| `tunnel.kcp.send_window` / `recv_window` | پنجره به تعداد بسته (پیش‌فرض ۱۰۲۴)؛ کوچک‌تر یعنی صف کمتر ولی سرعت کمتر زیر loss |
| `tunnel.kcp.mtu` | اندازه‌ی بسته‌ی KCP (پیش‌فرض ۱۳۵۰، حداکثر ۱۴۴۳ و با FEC ۱۴۲۹) |
| `tunnel.kcp.fec_data` / `fec_parity` | FEC برای سمت فرستنده، مثلاً ۱۰ و ۳؛ هر دو صفر (پیش‌فرض) یعنی خاموش |
| `tuning.keepalive_secs` | فاصله‌ی keepalive و ping در mux؛ پشت CDN حداکثر ۹۰ |
| `forward.protocol` | `tcp` (پیش‌فرض)، `udp` یا `tcp+udp`؛ برای UDP از mux استفاده کنید |
| `tuning.udp_timeout_secs` | flow بیکار UDP بعد از این مدت بسته می‌شود (پیش‌فرض ۶۰) |
| `tuning.udp_max_flows` | حداکثر flowهای UDP (آدرس‌های کلاینت) برای هر قانون (پیش‌فرض ۱۰۲۴) |

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
- UDP: حدود ۱۴۰ هزار بسته در ثانیه در هر جهت، در همه‌ی حالت‌ها
- تأخیر UDP: تانل بیکار حدود ۹۰ میکروثانیه به رفت‌وبرگشت اضافه می‌کند
- UDP زیر بار: با چهار انتقال حجیم TCP روی همان اتصال و لینک محدودشده به ۲۰ مگابیت، رفت‌وبرگشت UDP حدود ۶۰ میلی‌ثانیه می‌ماند (بدون تنظیم `TCP_NOTSENT_LOWAT` که کاریز روی اتصال‌های mux می‌گذارد، حدود ۳۴۰ میلی‌ثانیه بود)
- ۱۰۰۰ flow بیکار UDP: حدود ۳ تا ۳.۵ مگابایت حافظه‌ی اضافه برای هر سمت

### روی لینک پرتلفات

تست‌ها یک شبیه‌ساز لینک دارند (تأخیر، loss تصادفی، گلوگاه ۵۰ مگابیتی با صف ۵۰ میلی‌ثانیه‌ای). سمت TCP آن رفتار TCP فرستنده را مدل می‌کند، پس loss روی `tcpmux` همان اثری را دارد که روی مسیر واقعی. اندازه‌گیری روی runner‌ی GitHub Actions (۲ هسته‌ی AMD EPYC 7763)، با رفت‌وبرگشت ۶۰ میلی‌ثانیه و loss در هر دو جهت:

| transport | دانلود با loss صفر / ۱٪ / ۵٪ | p99 بسته‌های UDP روی تانل بیکار، loss ۱٪ / ۵٪ |
|---|---|---|
| `tcpmux` | 48.1 / 2.3 / 0.9 Mbit/s | 164 / 243 ms |
| `quic` (Cubic) | 47.9 / 2.7 / 1.0 Mbit/s | 64 / 64 ms (بسته‌ی گم‌شده دور ریخته می‌شود) |
| `quic` (BBR) | 47.5 / 47.4 / 45.7 Mbit/s | 64 / 64 ms |
| `kcp` | 31.4 / 28.5 / 22.5 Mbit/s | 141 / 202 ms |
| `kcp` با FEC 10/3 | 25.4 / 26.1 / 26.6 Mbit/s | 64 / 143 ms |

- کنترل ازدحام مبتنی بر loss (مثل Cubic) زیر loss تصادفی از سرعت می‌افتد، چه TCP چه QUIC. `quic` با BBR و `kcp` سرعتشان را حفظ می‌کنند.
- BBR (در quinn آزمایشی است) صف گلوگاه را سرریز می‌کند و هنگام دانلود حدود نیمی از بسته‌های UDP همان اتصال گم شد، برای همین پیش‌فرض Cubic است.
- پنجره‌ی پیش‌فرض KCP (۱۰۲۴ بسته) مسیرهای کوچک را سرریز می‌کند؛ پنجره‌ی نزدیک به «پهنای باند × RTT ÷ ۱۳۰۰» صف کمتری می‌سازد ولی زیر loss سرعت کمتری نگه می‌دارد.
- FEC روی این لینک سرعت را بیشتر نمی‌کند، ولی تأخیر بسته‌های پراکنده را زیر loss تقریباً نصف می‌کند.
- جزئیات و جدول کامل در [README انگلیسی](README.md#over-a-lossy-link) و [docs/PHASE4.md](docs/PHASE4.md) است.

روی localhost (همان runner)، `quic` حدود ۸۱۰ تا ۹۵۰ و `kcp` حدود ۶۰۰ تا ۶۷۰ مگابیت بر ثانیه می‌رسند، در مقابل ۲۹۶۰ تا ۳۴۲۰ برای `tcpmux`. QUIC و KCP پروتکلشان را در فضای کاربر و بسته به بسته اجرا می‌کنند؛ بین دو سرور این فقط بالای چند صد مگابیت اهمیت دارد. حافظه: در حالت بیکار حدود ۶٫۵ تا ۷٫۵ مگابایت برای هر سمت، و ۱۰۰ اتصال بیکار حدود ۱ مگابایت اضافه.

حجم فایل اجرایی (musl، x86_64): با همه‌ی امکانات 6.0 مگابایت؛ بدون `quic` و `kcp` (با `--no-default-features`) 4.5 مگابایت.

## نکته‌های امنیتی

- توکن تنها راز است و هر کسی آن را داشته باشد می‌تواند از تانل استفاده کند. آن را با `kariz token` بسازید و جایی به اشتراک نگذارید.
- `encryption = "none"` فقط احراز هویت می‌کند و ترافیک روی شبکه خواناست. فقط داخل یک لایه‌ی رمزشده‌ی دیگر از آن استفاده کنید.
- پشت CDN، اتصال TLS در خود CDN تمام می‌شود، اما رمزنگاری خود تانل همچنان محتوا را از CDN هم پنهان نگه می‌دارد.
- ClientHello کتابخانه‌ی rustls شبیه مرورگر نیست، و درخواست HTTP ساده به پورت `wss` به‌جای صفحه‌ی nginx خطای TLS می‌گیرد. handshake‌ی QUIC کتابخانه‌ی quinn هم همین‌طور است. این‌ها برای فاز ۶ (استتار) برنامه‌ریزی شده‌اند.
- پورت `kcp` به هیچ بسته‌ای که با توکن مهر نشده باشد جواب نمی‌دهد. کلید این لایه از توکن ساخته می‌شود و امنیت پیشرو ندارد؛ handshake و رمزنگاری خود تانل مثل روی TCP داخل آن اجرا می‌شوند.

## توسعه

<div dir="ltr">

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test                          # nginx tests run too when nginx is installed
cargo build --release --no-default-features   # without quic and kcp (features `quic`, `kcp`)
cargo test --release --test tunnel throughput -- --ignored --nocapture
cargo test --release --test tunnel lossy_link -- --ignored --nocapture   # KARIZ_BENCH_ONLY=kcp
scripts/rss.sh tcpmux 100           # memory, after cargo build --release
scripts/rss.sh tcpmux 1000 target/release/kariz udp   # memory per UDP flow
KARIZ_TEST_LOG=1 cargo test --test tunnel <name>        # with the tunnel's debug logs
```

</div>

مستندات طراحی: [docs/ROADMAP.md](docs/ROADMAP.md)، [docs/PHASE2.md](docs/PHASE2.md)، [docs/PHASE3.md](docs/PHASE3.md) و [docs/PHASE4.md](docs/PHASE4.md).

</div>
