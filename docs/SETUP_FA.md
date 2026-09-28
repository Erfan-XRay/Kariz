<div dir="rtl">

# راهنمای راه‌اندازی کاریز روی سرورهای لینوکس

این راهنما قدم‌به‌قدم کاریز را روی دو سرور واقعی راه می‌اندازد، اتصال را تست می‌کند و سرعت و تأخیرش را اندازه می‌گیرد. اول با ساده‌ترین حالت شروع می‌کنیم (`tcpmux`، حالت reverse). بعد که کار کرد، با عوض کردن چند خط سراغ `wss`، `quic` و `kcp` می‌رویم.

فهرست:

1. [پیش‌نیازها](#۱-پیشنیازها)
2. [نصب روی هر دو سرور](#۲-نصب-روی-هر-دو-سرور)
3. [همگام کردن ساعت سرورها](#۳-همگام-کردن-ساعت-سرورها)
4. [انتخاب حالت و transport](#۴-انتخاب-حالت-و-transport)
5. [راه‌اندازی اول: reverse با tcpmux](#۵-راهاندازی-اول-reverse-با-tcpmux)
6. [تست اتصال، سرعت و تأخیر](#۶-تست-اتصال-سرعت-و-تأخیر)
7. [استفاده‌ی واقعی: پروکسی و WireGuard](#۷-استفادهی-واقعی-پروکسی-و-wireguard)
8. [transportهای دیگر](#۸-transportهای-دیگر)
9. [تنظیمات سیستم برای کارایی بهتر](#۹-تنظیمات-سیستم-برای-کارایی-بهتر)
10. [عیب‌یابی](#۱۰-عیبیابی)
11. [به‌روزرسانی و حذف](#۱۱-بهروزرسانی-و-حذف)
12. [گزارش نتیجه‌ی تست](#۱۲-گزارش-نتیجهی-تست)

---

## ۱. پیش‌نیازها

- **دو سرور لینوکس x86_64**، مثلاً یکی در ایران (**entry**) و یکی خارج (**exit**). فایل اجرایی استاتیک است و روی هر توزیعی (Ubuntu، Debian، CentOS، Alma و...) بدون نصب چیز دیگری اجرا می‌شود.
- **دسترسی root** (یا sudo) روی هر دو.
- **ساعت همگام:** اختلاف ساعت دو سرور باید زیر **۲ دقیقه** باشد، وگرنه handshake رد می‌شود (بخش ۳).
- **یک پورت باز** روی سروری که گوش می‌دهد. برای شروع، پورت TCP ‏`3080` روی سرور ایران.

دو اصطلاح که همه‌جا تکرار می‌شوند:

| اصطلاح | معنی |
|---|---|
| **entry** | سروری که کاربرها به آن وصل می‌شوند (معمولاً ایران). قوانین `[[forward]]` فقط روی این سمت‌اند. |
| **exit** | سروری که به مقصد واقعی وصل می‌شود (معمولاً خارج). |
| حالت **reverse** | سرور exit به entry وصل می‌شود. وقتی خوب است که اتصال ورودی از خارج به ایران باز است. |
| حالت **direct** | سرور entry به exit وصل می‌شود. |

---

## ۲. نصب روی هر دو سرور

این دستورها را **روی هر دو سرور** اجرا کنید:

<div dir="ltr">

```bash
VERSION=v0.4.0
cd /tmp
curl -LO https://github.com/Erfan-XRay/Kariz/releases/download/$VERSION/kariz-$VERSION-x86_64-linux.tar.gz
curl -LO https://github.com/Erfan-XRay/Kariz/releases/download/$VERSION/kariz-$VERSION-x86_64-linux.tar.gz.sha256
sha256sum -c kariz-$VERSION-x86_64-linux.tar.gz.sha256      # must print: OK
tar xzf kariz-$VERSION-x86_64-linux.tar.gz
cd kariz-$VERSION-x86_64-linux

sudo install -m 755 kariz /usr/local/bin/kariz
sudo mkdir -p /etc/kariz
sudo cp -r configs /etc/kariz/samples
sudo cp systemd/kariz.service /etc/systemd/system/
sudo systemctl daemon-reload

kariz --version                                            # kariz 0.4.0
```

</div>

> اگر سرور ایران به GitHub دسترسی ندارد، فایل را روی سرور خارج دانلود کنید و با `scp` بفرستید:
> `scp kariz-v0.4.0-x86_64-linux.tar.gz root@IRAN_IP:/tmp/`

سرویس systemd برنامه را از `/usr/local/bin/kariz` و کانفیگ را از `/etc/kariz/config.toml` می‌خواند، بعد از هر قطعی دوباره اجرایش می‌کند و محدودیت فایل‌های باز را بالا می‌برد.

---

## ۳. همگام کردن ساعت سرورها

handshake‌ی کاریز زمان‌دار است تا نشود یک اتصال ضبط‌شده را دوباره فرستاد (replay). برای همین، اگر ساعت دو سرور بیشتر از **۱۲۰ ثانیه** فاصله داشته باشد، اتصال رد می‌شود و در لاگ `hello timestamp out of range` می‌آید.

روی هر دو سرور:

<div dir="ltr">

```bash
timedatectl                        # "System clock synchronized: yes" is what we want
sudo timedatectl set-ntp true      # turn on NTP if it is off
date -u                            # compare the two servers by eye
```

</div>

اگر NTP کار نمی‌کند (مثلاً پورت UDP 123 بسته است)، `chrony` نصب کنید یا ساعت را دستی تنظیم کنید: `sudo date -s "2026-09-28 12:00:00"`.

---

## ۴. انتخاب حالت و transport

**حالت:**

- **reverse** (پیشنهاد برای شروع): سرور خارج به سرور ایران وصل می‌شود. پورت فقط روی سرور ایران باز می‌شود و IP سرور خارج در هیچ کانفیگی روی سرور ایران نمی‌آید.
- **direct:** سرور ایران به سرور خارج وصل می‌شود. وقتی لازم است که اتصال ورودی به سرور ایران بسته باشد، یا وقتی از CDN جلوی سرور خارج استفاده می‌کنید.

**transport:**

| transport | روی شبکه | کِی |
|---|---|---|
| `tcpmux` | چند اتصال TCP بلندمدت | **شروع کار**؛ ساده و سریع |
| `tcp` | یک اتصال TCP برای هر اتصال کاربر | ساده‌ترین حالت؛ بدون mux |
| `ws` / `wss` | WebSocket (با TLS) | پشت CDN؛ یا برای اینکه شبیه یک سایت HTTPS باشد |
| `quic` | QUIC روی UDP | UDP مثل بازی و WireGuard بدون معطلی پشت بسته‌ی گم‌شده؛ فقط وقتی UDP روی مسیر خوب کار می‌کند |
| `kcp` | KCP روی UDP، هر بسته رمزشده | لینک پرتلفات که TCP در آن کُند می‌شود؛ فقط وقتی UDP روی مسیر خوب کار می‌کند |

روی لینک پرتلفات (نتیجه‌ی بنچمارک با RTT ‏۶۰ms): با ۱٪ loss، ‏`tcpmux` حدود ۲ مگابیت می‌گیرد، `kcp` حدود ۲۸ مگابیت و `quic` با BBR حدود ۴۷ مگابیت. اما UDP در بعضی شبکه‌های ایران محدود یا مسدود است. **پس اول با `tcpmux` مطمئن شوید همه چیز کار می‌کند، بعد transportهای UDP را امتحان کنید.** عوض کردن transport فقط چند خط کانفیگ است.

---

## ۵. راه‌اندازی اول: reverse با tcpmux

### ۵.۱ توکن

روی یکی از سرورها یک توکن بسازید و **همان** را در کانفیگ هر دو سرور بگذارید:

<div dir="ltr">

```bash
kariz token
```

</div>

توکن تنها رمز تانل است؛ هر کسی آن را داشته باشد می‌تواند از تانل استفاده کند. آن را جایی به اشتراک نگذارید.

### ۵.۲ کانفیگ سرور ایران (entry)

فایل `/etc/kariz/config.toml` روی سرور ایران:

<div dir="ltr">

```toml
role = "entry"
mode = "reverse"
profile = "balanced"

[tunnel]
transport = "tcpmux"
listen = "0.0.0.0:3080"            # the exit server connects here (TCP)
token = "PUT-YOUR-TOKEN-HERE"

# For the tests of section 6: iperf3 (TCP and UDP) and a small web page,
# both running on the exit server. Replace with your real services later (section 7).
[[forward]]
listen = "0.0.0.0:5201"
target = "127.0.0.1:5201"
protocol = "tcp+udp"

[[forward]]
listen = "0.0.0.0:8080"
target = "127.0.0.1:8080"

[log]
level = "info"
```

</div>

`target` از دید **سرور exit** است: `127.0.0.1:5201` یعنی پورت 5201 روی خود سرور خارج.

### ۵.۳ کانفیگ سرور خارج (exit)

فایل `/etc/kariz/config.toml` روی سرور خارج:

<div dir="ltr">

```toml
role = "exit"
mode = "reverse"
profile = "balanced"

[tunnel]
transport = "tcpmux"
remote = "IRAN_SERVER_IP:3080"
token = "PUT-YOUR-TOKEN-HERE"

[log]
level = "info"
```

</div>

### ۵.۴ فایروال

فقط سرور ایران به پورت ورودی نیاز دارد: TCP ‏`3080` برای تانل، و پورت‌های `[[forward]]` برای کاربرها. با `ufw`:

<div dir="ltr">

```bash
sudo ufw allow 3080/tcp          # tunnel
sudo ufw allow 5201              # test forward (tcp and udp)
sudo ufw allow 8080/tcp          # test forward
```

</div>

(با `firewalld`: `sudo firewall-cmd --permanent --add-port=3080/tcp && sudo firewall-cmd --reload`.) اگر ارائه‌دهنده‌ی سرور فایروال جدا در پنل دارد، آنجا هم باز کنید.

### ۵.۵ بررسی و اجرا

روی هر دو سرور، اول کانفیگ را بررسی کنید:

<div dir="ltr">

```bash
kariz check -c /etc/kariz/config.toml
```

</div>

باید `config OK` و خلاصه‌ای از تنظیمات چاپ شود. هشدارها را جدی بگیرید. بعد اجرا کنید، **اول سرور ایران** (که گوش می‌دهد):

<div dir="ltr">

```bash
sudo systemctl enable --now kariz
journalctl -u kariz -f
```

</div>

**چیزی که در لاگ باید ببینید:**

- سرور ایران: `entry: reverse mode, waiting for the exit side`، یک خط `forward ready` برای هر قانون `[[forward]]`، و بعد از بالا آمدن سرور خارج، چند خط `mux session from the exit side established` (پیش‌فرض ۴ تا).
- سرور خارج: `exit: reverse mode with mux, keeping sessions to the entry side` و چند خط `mux session to the entry side established`.

اگر این خط‌ها نیامد، [بخش ۱۰](#۱۰-عیبیابی) را ببینید.

---

## ۶. تست اتصال، سرعت و تأخیر

### ۶.۱ تست ساده با یک صفحه‌ی وب

روی **سرور خارج** یک وب‌سرور موقت روی localhost بالا بیاورید:

<div dir="ltr">

```bash
mkdir -p /tmp/www && echo "kariz works" > /tmp/www/index.html
cd /tmp/www && python3 -m http.server 8080 --bind 127.0.0.1
```

</div>

از هر جایی (مثلاً کامپیوتر خودتان):

<div dir="ltr">

```bash
curl http://IRAN_SERVER_IP:8080/
```

</div>

اگر `kariz works` برگشت، تانل کار می‌کند: درخواست از سرور ایران، از داخل تانل، به localhost‌ی سرور خارج رسیده است.

### ۶.۲ سرعت با iperf3

روی هر دو سرور `iperf3` نصب کنید (`apt install iperf3` یا `dnf install iperf3`). روی **سرور خارج**:

<div dir="ltr">

```bash
iperf3 -s -B 127.0.0.1 -p 5201
```

</div>

از **سرور ایران** (یا از یک کلاینت دیگر، با IP سرور ایران به‌جای 127.0.0.1):

<div dir="ltr">

```bash
iperf3 -c 127.0.0.1 -p 5201 -t 20          # upload: Iran -> abroad
iperf3 -c 127.0.0.1 -p 5201 -t 20 -R       # download: abroad -> Iran (what users mostly do)
iperf3 -c 127.0.0.1 -p 5201 -t 20 -R -P 4  # four parallel streams
```

</div>

برای مقایسه، سرعت **بدون تانل** را هم بگیرید: یک `iperf3 -s` بدون `-B` روی سرور خارج (با پورت آزاد در فایروال)، و از سرور ایران `iperf3 -c ABROAD_SERVER_IP -R`. اختلاف این دو، هزینه‌ی تانل است. اگر مسیر مستقیم فیلتر یا کُند است، ممکن است تانل حتی سریع‌تر باشد.

> **سقف یک stream:** با پروفایل `balanced` پنجره‌ی هر stream ‏۲۵۶ کیلوبایت است. یعنی یک اتصال تکی روی مسیری با RTT ‏۱۰۰ms حداکثر حدود ۲۰ مگابیت می‌گیرد. اگر یک اتصال تکی کُند بود ولی `-P 4` سریع، `profile = "throughput"` (یک مگابایت) یا `[tunnel.mux] stream_window = 1048576` را امتحان کنید.

### ۶.۳ UDP و تأخیر

UDP از همان قانون `tcp+udp` رد می‌شود:

<div dir="ltr">

```bash
iperf3 -c 127.0.0.1 -p 5201 -u -b 20M -t 20 -R     # loss and jitter at 20 Mbit/s
```

</div>

در خروجی، ستون‌های `Jitter` و `Lost/Total` مهم‌اند. تأخیر را با `ping` از سرور ایران به سرور خارج بسنجید (بیرون از تانل). تانل روی سرور بیکار فقط حدود ۰٫۱ میلی‌ثانیه اضافه می‌کند.

### ۶.۴ پاک کردن تست‌ها

وقتی تست تمام شد، وب‌سرور و `iperf3 -s` را متوقف کنید (`Ctrl+C`). قانون‌های `[[forward]]` تست را با سرویس‌های واقعی عوض کنید و پورت‌های تست را در فایروال ببندید.

---

## ۷. استفاده‌ی واقعی: پروکسی و WireGuard

کاریز فقط پورت‌ها را جابه‌جا می‌کند: هر اتصالی که به `listen` روی سرور ایران برسد، روی سرور خارج به `target` وصل می‌شود. پس هر سرویسی که روی سرور خارج دارید، از روی سرور ایران در دسترس می‌شود.

**پروکسی (Xray / V2Ray / sing-box و...)** که روی سرور خارج روی پورت 443 گوش می‌دهد:

<div dir="ltr">

```toml
[[forward]]
listen = "0.0.0.0:443"             # clients connect to IRAN_SERVER_IP:443
target = "127.0.0.1:443"           # the proxy inbound on the exit server
```

</div>

کلاینت‌ها به‌جای IP سرور خارج، IP سرور ایران را می‌گذارند. بقیه‌ی تنظیمات پروکسی (UUID، SNI و...) عوض نمی‌شود.

**WireGuard** (UDP) روی سرور خارج روی پورت 51820:

<div dir="ltr">

```toml
[[forward]]
listen = "0.0.0.0:51820"
target = "127.0.0.1:51820"
protocol = "udp"
```

</div>

برای UDP حتماً mux روشن باشد (`tcpmux`، `ws`، `wss`، `kcp` یا `quic`). بدون mux، هر کلاینت UDP یک اتصال تانل جدا می‌گیرد. `kariz check` در این حالت هشدار می‌دهد.

قانون‌ها را فقط روی سرور ایران (entry) عوض کنید. بعد از هر تغییر کانفیگ: `sudo systemctl restart kariz`.

---

## ۸. transportهای دیگر

در همه‌ی این حالت‌ها: `transport` و بخش‌های مربوط باید **در هر دو سمت یکی** باشد، و بعد از تغییر، هر دو سرویس را restart کنید.

### ۸.۱ `quic` (روی UDP)

در کانفیگ **هر دو** سرور، `transport = "tcpmux"` را به `"quic"` تغییر دهید. بخش `[tunnel.mux]` را اگر دارید نگه دارید یا پاک کنید؛ برای QUIC همیشه روشن است. در فایروال سرور ایران، پورت تانل را برای **UDP** باز کنید:

<div dir="ltr">

```bash
sudo ufw allow 3080/udp
```

</div>

اختیاری، در هر دو سمت:

<div dir="ltr">

```toml
[tunnel.quic]
congestion = "cubic"      # cubic (default) | bbr | newreno
```

</div>

- **`bbr`** روی لینک پرتلفات سرعت را خیلی بهتر نگه می‌دارد، ولی در quinn آزمایشی است و صف مسیر را پر می‌کند. در بنچمارک، هنگام دانلود حدود نیمی از بسته‌های UDP روی همان اتصال گم شد. برای دانلود حجیم بدون ترافیک بلادرنگ (بازی، تماس) امتحانش کنید.
- **WireGuard روی QUIC:** MTU کلاینت WireGuard را روی **1280** بگذارید تا بسته‌هایش در یک datagram جا شوند. بسته‌های بزرگ‌تر هم می‌رسند، ولی از مسیر کُندتر.
- در `quic`، رمزنگاری همیشه TLS 1.3 خود QUIC است؛ `encryption` را روی `auto` بگذارید.

نمونه‌ی کامل: `configs/entry-quic-direct.toml` و `configs/exit-quic-direct.toml`.

### ۸.۲ `kcp` (روی UDP، برای لینک پرتلفات)

در کانفیگ **هر دو** سرور، `transport = "kcp"` بگذارید و پورت تانل را در فایروال سرور ایران برای **UDP** باز کنید (`sudo ufw allow 3080/udp`). اختیاری، در هر دو سمت:

<div dir="ltr">

```toml
[tunnel.kcp]
mode = "fast2"            # normal | fast | fast2 (default) | fast3
# send_window = 1024      # packets
# recv_window = 1024
# fec_data = 10           # FEC: 3 parity packets per 10 data packets
# fec_parity = 3
```

</div>

- **پنجره:** پیش‌فرض ۱۰۲۴ بسته است. روی مسیرهای کم‌ظرفیت صف را پر می‌کند و تأخیر بالا می‌رود. عدد مناسب حدوداً «پهنای باند (بایت در ثانیه) × RTT ÷ ۱۳۰۰» است؛ مثلاً برای ۵۰ مگابیت و RTT ‏۶۰ms حدود ۳۰۰. پنجره‌ی کوچک‌تر یعنی تأخیر کمتر، ولی زیر loss سرعت کمتری نگه می‌دارد. با iperf3 چند عدد را امتحان کنید.
- **FEC:** بسته‌ی گم‌شده را بدون ارسال دوباره بازسازی می‌کند. تأخیر ترافیک پراکنده (بازی، ping) را زیر loss تقریباً نصف می‌کند، ولی ۳۰٪ بسته‌ی بیشتر می‌فرستد و سرعت دانلود را بالا نمی‌برد. تنظیم FEC برای هر سمت جداست و مخصوص جهت ارسال همان سمت است.
- هر بسته‌ی KCP با کلیدی از توکن رمز می‌شود. پورت به هیچ بسته‌ای که با توکن درست ساخته نشده باشد جواب نمی‌دهد.

نمونه‌ی کامل: `configs/entry-kcp-reverse.toml` و `configs/exit-kcp-reverse.toml`.

### ۸.۳ `wss` (شبیه یک سایت HTTPS)

در این حالت سمتی که گوش می‌دهد گواهی TLS لازم دارد. برای شروع، یک گواهی self-signed روی **سرور ایران** (در حالت reverse گوش می‌دهد) بسازید:

<div dir="ltr">

```bash
sudo openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 \
  -subj "/CN=www.example.com" -addext "subjectAltName=DNS:www.example.com" \
  -keyout /etc/kariz/key.pem -out /etc/kariz/cert.pem
kariz pin /etc/kariz/cert.pem            # prints the fingerprint for pin_sha256
```

</div>

سرور ایران (entry، گوش می‌دهد):

<div dir="ltr">

```toml
[tunnel]
transport = "wss"
listen = "0.0.0.0:443"
token = "PUT-YOUR-TOKEN-HERE"

[tunnel.ws]
path = "/ws"

[tunnel.tls]
cert = "/etc/kariz/cert.pem"
key = "/etc/kariz/key.pem"
```

</div>

سرور خارج (exit، وصل می‌شود):

<div dir="ltr">

```toml
[tunnel]
transport = "wss"
remote = "IRAN_SERVER_IP:443"
token = "PUT-YOUR-TOKEN-HERE"

[tunnel.ws]
path = "/ws"
host = "www.example.com"
early_data = true

[tunnel.tls]
pin_sha256 = "OUTPUT-OF-KARIZ-PIN"
```

</div>

`path` باید در هر دو سمت یکی باشد. هر درخواستی به مسیر دیگر، صفحه‌ی 404 یک nginx معمولی را می‌گیرد. برای استفاده از **CDN** (Cloudflare و مانند آن) راهنمای جدا در [docs/CDN.md](CDN.md) است.

### ۸.۴ حالت direct

اگر می‌خواهید سرور ایران به خارج وصل شود، `mode = "direct"` را در **هر دو** سمت بگذارید و جای `listen` و `remote` را عوض کنید:

- سرور ایران (entry): `remote = "ABROAD_SERVER_IP:3080"`
- سرور خارج (exit): `listen = "0.0.0.0:3080"`

حالا پورت تانل باید روی **سرور خارج** در فایروال باز باشد. قوانین `[[forward]]` مثل قبل روی سرور ایران می‌مانند.

---

## ۹. تنظیمات سیستم برای کارایی بهتر

این‌ها اختیاری‌اند ولی روی لینک‌های واقعی فرق می‌کنند. روی **هر دو** سرور:

<div dir="ltr">

```bash
sudo tee /etc/sysctl.d/90-kariz.conf >/dev/null <<'EOF'
# Bigger UDP socket buffers: quic and kcp ask for 1-4 MiB, but Linux caps them at
# net.core.rmem_max / wmem_max (about 208 KiB by default), and a full buffer drops packets.
net.core.rmem_max = 8388608
net.core.wmem_max = 8388608
# BBR for the kernel's TCP (tcp, tcpmux, ws, wss): keeps its rate under random loss
# far better than the default Cubic.
net.core.default_qdisc = fq
net.ipv4.tcp_congestion_control = bbr
EOF
sudo sysctl --system
sysctl net.ipv4.tcp_congestion_control      # should print bbr
```

</div>

- **BBR** (نیاز به کرنل ۴٫۹ یا جدیدتر) مهم‌ترین تنظیم برای transportهای TCP روی مسیرهای پرتلفات است. در بنچمارک خود کاریز، TCP با Cubic زیر ۱٪ loss به حدود ۲ مگابیت افت کرد. BBR درست همین مشکل را حل می‌کند. تأثیرش را روی مسیر خودتان با iperf3، قبل و بعد، بسنجید.
- **پروفایل‌ها:** `balanced` پیش‌فرض است. `throughput` بافرها و پنجره‌های بزرگ‌تری دارد (دانلود حجیم)، و `gaming` بافرهای کوچک‌تر و keepalive کوتاه‌تر (تأخیر کمتر). در هر دو سمت یکسان بگذارید.

---

## ۱۰. عیب‌یابی

اول لاگ هر دو سمت را ببینید: `journalctl -u kariz -f`. برای جزئیات بیشتر، سطح لاگ را بالا ببرید:

<div dir="ltr">

```bash
sudo systemctl edit kariz
# add these two lines, save:
#   [Service]
#   Environment=RUST_LOG=kariz=debug
sudo systemctl restart kariz
```

</div>

| نشانه (در لاگ) | علت محتمل | راه‌حل |
|---|---|---|
| سرور خارج: `could not connect to the entry side` با `Connection refused` یا `timed out` | پورت تانل بسته است، IP یا پورت اشتباه است، یا سرور ایران اجرا نمی‌شود | `ss -tlnp \| grep 3080` روی سرور ایران؛ فایروال سرور و پنل ارائه‌دهنده؛ از سرور خارج: `nc -vz IRAN_SERVER_IP 3080` |
| سرور ایران: `tunnel handshake failed` همراه با `hello timestamp out of range` | اختلاف ساعت بیش از ۲ دقیقه | بخش ۳ |
| سرور خارج: `could not connect to the entry side` با `handshake timed out`، و در ایران `tunnel handshake failed` | توکن دو سمت یکی نیست، یا mux یک طرف روشن و طرف دیگر خاموش است. سمت گوش‌دهنده به اتصالی که احراز هویت نشود جواب نمی‌دهد، پس سمت دیگر فقط timeout می‌بیند | توکن را دوباره در هر دو کپی کنید؛ `transport` و `[tunnel.mux]` را دو طرف یکی کنید. پیام روشن‌تر در لاگ سمت گوش‌دهنده است |
| `... rejected the tunnel connection` | سمت مقابل transport یا cipher این سمت را نمی‌پذیرد | `transport` و `encryption` را در دو سمت یکی کنید |
| سرور ایران: `no tunnel connection available (is the exit side running?)` | سرور خارج هنوز وصل نشده یا اتصالش قطع شده | لاگ سرور خارج را ببینید |
| سرور خارج: `could not connect to target` | `target` در قانون `[[forward]]` روی سرور خارج در دسترس نیست | سرویس مقصد روی سرور خارج بالاست؟ `ss -tlnp` |
| `quic` یا `kcp` وصل نمی‌شود، ولی `tcpmux` کار می‌کند | UDP روی مسیر بسته یا محدود است، یا پورت UDP در فایروال باز نیست | `ufw allow 3080/udp`؛ اگر باز هم نشد، مسیر UDP را نمی‌پذیرد و با `tcpmux` یا `wss` بمانید |
| `wss`: اتصال برقرار نمی‌شود و خطای TLS | `pin_sha256` با گواهی سرور نمی‌خواند | `kariz pin` را دوباره روی گواهی فعلی اجرا کنید |
| `too many UDP flows` | تعداد کلاینت‌های UDP از سقف گذشته | `[tuning] udp_max_flows` را بالا ببرید |
| سرعت یک اتصال کم است ولی چند اتصال موازی خوب است | سقف پنجره‌ی stream | `profile = "throughput"` یا `stream_window` بزرگ‌تر (بخش ۶.۲) |
| سرعت روی لینک پرتلفات خیلی پایین است | کنترل ازدحام TCP زیر loss | BBR (بخش ۹)، یا `kcp` / `quic` با BBR |

ابزارهای مفید دیگر:

<div dir="ltr">

```bash
systemctl status kariz                       # is it running, last lines of the log
ss -tnp | grep kariz                         # the tunnel's TCP connections
ss -unp | grep kariz                         # UDP sockets (quic, kcp, UDP forwards)
kariz check -c /etc/kariz/config.toml        # after every config change
```

</div>

---

## ۱۱. به‌روزرسانی و حذف

**به‌روزرسانی:** بخش ۲ را با نسخه‌ی جدید تکرار کنید (فقط `install` فایل اجرایی لازم است) و سرویس را restart کنید. برای `quic` و `kcp` هر دو سمت باید نسخه‌ی یکسان داشته باشند. `tcp`، `tcpmux`، `ws` و `wss` در 0.4 با 0.3 سازگارند. تغییرات هر نسخه در [CHANGELOG.md](../CHANGELOG.md) است.

**حذف:**

<div dir="ltr">

```bash
sudo systemctl disable --now kariz
sudo rm /etc/systemd/system/kariz.service /usr/local/bin/kariz
sudo rm -r /etc/kariz
sudo systemctl daemon-reload
```

</div>

---

## ۱۲. گزارش نتیجه‌ی تست

اگر نتیجه‌ی تست را برای بررسی می‌فرستید، این‌ها بیشترین کمک را می‌کنند:

- موقعیت دو سرور (مثلاً ایران، دیتاسنتر X ↔ آلمان)، ارائه‌دهنده‌ها، و `ping` بین آن‌ها
- `transport`، `mode` و `profile`، و اینکه BBR (بخش ۹) روشن بود یا نه
- خروجی iperf3 با تانل و بدون تانل: `-R`، `-R -P 4` و `-u -b 20M -R`
- برای `quic` و `kcp`: کار کرد یا نه، و اگر کار کرد همان سه عدد
- هر خط `warn` یا `error` از `journalctl -u kariz` در دو سمت
- مصرف منابع در حین تست: `top -p $(pidof kariz)` (CPU و RES)

</div>
