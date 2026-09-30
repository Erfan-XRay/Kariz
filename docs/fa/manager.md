# اسکریپت مدیر

[English](../manager.md) · **فارسی**

`scripts/kariz.sh` کاریز را نصب می‌کند و تانل‌هایش را روی یک سرور لینوکس با systemd راه می‌اندازد و اداره می‌کند. بعد از اولین اجرا، همان فرمان `kariz-manager` هم هست.

## نصب

با دسترسی root، روی هر سرور:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh)
```

یک منو باز می‌شود. **۱** را برای نصب بزنید: CPU شما را پیدا می‌کند (x86_64، aarch64 یا armv7)، آخرین ریلیز را دانلود می‌کند، **امضا** و SHA-256 آن را می‌سنجد و `kariz` را در `/usr/local/bin`، قالب systemd به نام `kariz@.service` و فرمان `kariz-manager` را نصب می‌کند.

راه‌های دیگر نصب:

```bash
kariz-manager install --version v0.11.0       # a specific release
kariz-manager install --binary ./kariz        # a binary you built yourself
```

## تانل‌ها چطور نگه داشته می‌شوند

هر تانل یک نام دارد. تنظیم‌هایش `/etc/kariz/<name>.toml` است (فقط برای root خواندنی، چون token در آن است) و systemd آن را به‌صورت سرویس `kariz@<name>` اجرا می‌کند. پس یک سرور می‌تواند چند تانل داشته باشد، هرکدام موقع بوت شروع می‌شود و اگر بایستد دوباره راه می‌افتد.

## افزودن تانل

در منو **New tunnel** را بزنید. شش مرحله دارد. انتخاب‌ها فهرست شماره‌دارند: شماره (یا کلمه) را بنویسید یا Enter بزنید تا مقدار پیش‌فرضِ داخل کروشه بیاید. Ctrl+C در هر لحظه لغو می‌کند و به منو برمی‌گردد، بدون اینکه چیزی نوشته شود.

| مرحله | می‌پرسد |
|---|---|
| ۱. نام | حرف، رقم، `-` و `_` |
| ۲. سمت این سرور | entry (کاربرها اینجا وصل می‌شوند) یا exit (به مقصدها می‌رسد)؛ reverse (exit به entry وصل می‌شود) یا direct |
| ۳. ترنسپورت و پروفایل | با راهنمای یک‌خطی برای هرکدام |
| ۴. اتصال | سمت شنونده: یک پورت (بررسی می‌شود آزاد باشد)، IPv4 و IPv6 یا فقط IPv4، و آدرسی که سرور دیگر به آن وصل می‌شود (IPv4 یا IPv6 این سرور، یا آدرس یا دامنهٔ دیگر). سمت وصل‌شونده: آدرس سرور دیگر |
| ۵. امنیت | token تازه یا چسباندن token سرور دیگر؛ مسیر WebSocket (`ws` / `wss`)؛ pin گواهی (سمت وصل‌شوندهٔ `wss`) |
| ۶. پورت‌های فوروارد | فقط entry: پروتکل، میزبان مقصد آن‌طور که exit می‌بیند (پیش‌فرض `127.0.0.1`) و پورت‌ها. برای پروتکل یا مقصد دیگر تکرار کنید |

بعد یک خلاصه نشان می‌دهد و پیش از ساختن هر چیزی می‌پرسد.

### پورت‌ها

پورت‌ها یک فهرست‌اند، با ویرگول جدا:

| بنویسید | یعنی |
|---|---|
| `443` | پورت ۴۴۳ اینجا، به پورت ۴۴۳ مقصد |
| `443,8443,2083` | چند پورت |
| `8080-8090` | یک بازه: هر پورت به همان پورت در مقصد |
| `2053=53` (یا `2053:53`) | کاربرها به ۲۰۵۳ اینجا وصل می‌شوند؛ exit به ۵۳ وصل می‌شود |
| `3000-3005=4000-4005` | یک بازه روی بازهٔ دیگری هم‌اندازه |
| `5000-5010=443` | چند پورت روی یک پورت |

قاطی هم می‌شوند: `443, 8080-8090, 2053=53`. اسکریپت بررسی می‌کند هیچ پورتی دو بار نیامده و هیچ‌کدام روی سرور در حال استفاده نیست. یک تانل تا ۱٬۰۰۰ پورت فوروارد می‌کند؛ هر کدام یک قانون `[[forward]]` در کانفیگ می‌شود.

### IPv4 و IPv6

هرجا آدرس می‌خواهد، IPv4، IPv6 و دامنه همه کار می‌کنند، با پورت یا بدون آن: `203.0.113.5`، `203.0.113.5:3080`، `2001:db8::1`، `[2001:db8::1]:3080`، `tunnel.example.com`. بدون پورت، پیش‌فرض تانل یعنی ۳۰۸۰ می‌آید. آدرس IPv6 در کانفیگ با کروشه نوشته می‌شود.

روی سروری که IPv6 دارد، تانل و پورت‌های فوروارد پیش‌فرض روی `[::]` گوش می‌دهند که هر دو IPv4 و IPv6 را می‌گیرد. «فقط IPv4» (یا `--ipv4-only`) را بزنید تا به‌جایش روی `0.0.0.0` گوش دهد. دو سرور می‌توانند با IPv6 با هم حرف بزنند و کاربرها با IPv4 وصل شوند، یا برعکس.

### سرور دیگر

پیش از شروع هر چیزی، اسکریپت فایل را می‌نویسد و `kariz check` را روی آن اجرا می‌کند. اگر کاریز رد کند خطا را نشان می‌دهد و چیزی عوض نمی‌شود. بعد فرمان سرور دیگر را آمادهٔ چسباندن چاپ می‌کند:

```text
On the other server, run:

  kariz-manager add main --role exit --mode reverse --transport tcpmux \
      --profile balanced --token <the token> --remote 203.0.113.5:3080
```

آن را روی سرور دیگر (بعد از نصب آنجا) اجرا کنید و جفت بالا می‌آید. همین فرمان `add` بدون منو هم برای خودکارسازی کار می‌کند:

```bash
kariz-manager add main --role entry --mode reverse --transport tcpmux \
    --listen 3080 --ports 443,8080-8090,2053=53
kariz-manager add games --role entry --mode reverse --transport kcp --profile gaming \
    --listen 3081 --ports 27015-27020 --protocol udp --to 10.0.0.5
```

| گزینه | معنی |
|---|---|
| `--role entry\|exit`، `--mode reverse\|direct` | مثل کانفیگ |
| `--transport` | `tcp`، `tcpmux`، `ws`، `wss`، `quic` یا `kcp` |
| `--profile` | `balanced` (پیش‌فرض)، `ultraspeed` یا `gaming` |
| `--listen PORT` / `--remote ADDR[:PORT]` | سمت شنونده `--listen` می‌دهد (یک پورت، یا `ADDR:PORT`)، سمت وصل‌شونده `--remote` |
| `--ports LIST` | فقط entry: پورت‌ها مثل بخش «پورت‌ها» بالا؛ می‌تواند تکرار شود |
| `--protocol tcp\|udp\|tcp+udp` | برای `--ports` (پیش‌فرض `tcp`) |
| `--to HOST` | میزبان مقصد برای `--ports`، آن‌طور که exit می‌بیند (پیش‌فرض `127.0.0.1`) |
| `--ipv4-only` | فقط روی IPv4 گوش بده |
| `--forward LISTEN=TARGET[/tcp\|udp\|tcp+udp]` | یک قانون کامل؛ می‌تواند تکرار شود |
| `--token` | پیش‌فرض: یک token تصادفی تازه |
| `--ws-path`، `--pin` | برای `ws` / `wss`؛ `--pin` برای سمت وصل‌شوندهٔ `wss` |
| `--public-ip` | آدرسی که در فرمان سمت دیگر چاپ شود (پیش‌فرض: آدرس این سرور) |

### `wss`

اول سمت **شنونده** را اضافه کنید. اسکریپت برایش یک گواهی خودامضا می‌سازد (`/etc/kariz/<name>.crt` و `.key`) و pin گواهی را در فرمانی که برای سمت وصل‌شونده چاپ می‌کند می‌گذارد، پس لازم نیست چیزی را دستی کپی کنید.

### پورت را باز کنید

اسکریپت به فایروال دست نمی‌زند. یادآوری می‌کند کدام پورت را باز کنید: TCP برای `tcp`، `tcpmux`، `ws` و `wss`، UDP برای `quic` و `kcp`، و پورت‌های فوروارد روی entry. security groupهای سرویس‌دهنده‌های ابری هم همین قاعده‌ها را لازم دارند.

## مدیریت تانل‌ها

بالای منو نسخهٔ نصب‌شده و تعداد تانل‌های در حال اجرا را نشان می‌دهد. عمل‌های روی یک تانل (شروع و توقف، لاگ، تست سرعت، ویرایش، حذف) تانل‌ها را شماره‌دار فهرست می‌کنند: شماره یا نام را بنویسید.

**Ctrl+C** وسط یک عمل (سؤال، لاگ، تست سرعت) مستقیم به منو برمی‌گردد. در خودِ منو از مدیر خارج می‌شود (`0` هم همین را می‌کند).

```bash
kariz-manager list                  # name, side, transport, ports, state, address
kariz-manager status main           # connection, round trip, traffic per port
kariz-manager status                # the same for every tunnel
kariz-manager logs main             # follows the log; Ctrl-C to stop
kariz-manager speedtest main        # speed, latency and UDP; on the entry server
kariz-manager restart main          # also: start, stop
kariz-manager edit main             # opens $EDITOR (nano, else vi)
kariz-manager remove main
```

- **`status`** برای تانل `kariz status` را اجرا می‌کند (روی هر دو سرور؛ `--watch` عبور می‌کند)، یا بدون نام برای همهٔ تانل‌ها. تانل متوقف به‌جایش دید systemd را با دلیل ایستادنش نشان می‌دهد. [Status](../status.md) را ببینید.
- **`speedtest`** برای تانل `kariz speedtest` را اجرا می‌کند (گزینه‌ها عبور می‌کنند، مثلاً `--seconds 5`). روی سرور entry و وقتی تانل در حال اجراست کار می‌کند؛ [تست سرعت](../speedtest.md).
- **`stop`** تانل را موقع بوت هم غیرفعال می‌کند؛ **`start`** دوباره روشنش می‌کند.
- **`edit`** پیش از اعمال، تغییر شما را با `kariz check` بررسی می‌کند. اگر بررسی رد شود تانل تنظیم‌های قبلی‌اش را نگه می‌دارد. بعد از یک ویرایش درست، تانل را دوباره راه می‌اندازد.
- **`remove`** تانل را متوقف و کانفیگ، گواهی و token آن را حذف می‌کند. `--yes` سؤال را رد می‌کند.

## به‌روزرسانی و حذف

```bash
kariz-manager update                # latest release; running tunnels are restarted
kariz-manager update --version v0.11.0
kariz-manager uninstall             # asks before removing configs; --yes removes everything
```

`install` و `update` **امضا** (از ۰٫۱۱ به بعد ریلیزی که امضای معتبر نداشته باشد نصب نمی‌شود؛ ریلیزهای قدیمی‌تر امضا ندارند و با هشدار پذیرفته می‌شوند) و SHA-256 ریلیز را می‌سنجند. `update` همهٔ تانل‌های در حال اجرا را دوباره راه می‌اندازد تا فایل اجرایی تازه را بگیرند. خودِ اسکریپت مدیر را عوض نمی‌کند؛ برای نسخهٔ تازهٔ مدیر دوباره بگیریدش:

```bash
curl -fsSL -o /usr/local/bin/kariz-manager \
    https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh
chmod +x /usr/local/bin/kariz-manager
```

## لاگ‌ها

لاگ کاریز به journal می‌رود، با اولویت systemd برای هر خط؛ پس:

```bash
journalctl -u kariz@main -f                 # follow
journalctl -u kariz@main -p warning         # only warnings and errors
```

## خوب است بدانید

- root، systemd و `curl` یا `wget` لازم دارد. `wss` `openssl` هم می‌خواهد.
- منو و فرمان‌ها همان کارها را می‌کنند؛ هرکدام را که خواستید.
- کانفیگ‌هایی که اسکریپت می‌نویسد کانفیگ معمولی کاریزند. دستی ویرایششان کنید و برای هر تنظیم [مرجع کانفیگ](../configuration.md) را ببینید.
- unit سرویس `systemd/kariz@.service` است. `systemd/kariz.service` قدیمی‌تر (یک تانل، `/etc/kariz/config.toml`) برای راه‌اندازی دستی هنوز کار می‌کند.

## پنل وب و agentها

مدیر [پنل وب](panel.md) را هم نصب می‌کند و سرورها را به آن وصل می‌کند:

```bash
kariz-manager panel install         # the panel on this server: address, certificate, a login link
kariz-manager panel link            # another one-time login link
kariz-manager panel password        # a new admin password
kariz-manager --agent kz1_...       # connect this server to a panel (the code is from the panel)
kariz-manager agent status          # also: logs, remove
kariz-manager net list              # the private network (GRE) links of this server
```

منو آن‌ها را زیر *w* دارد. `install` و `update` برنامهٔ پنل را که با آرشیو ریلیز می‌آید (از ۰٫۸) هم می‌گیرند و `update` پنل و agent را هم دوباره راه می‌اندازد.
