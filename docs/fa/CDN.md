# اجرای کاریز پشت CDN

[English](../CDN.md) · **فارسی**

CDN سرور تانل را پشت آدرس‌های IP خودش پنهان می‌کند. DPI فقط یک اتصال TLS به یک edge از CDN را می‌بیند، با SNI یک وب‌سایت عادی، که یک WebSocket را می‌برد. این صفحه Cloudflare (که اول آزمایش شد) را پوشش می‌دهد و برای ArvanCloud نکته‌هایی دارد. فهرست دستی آخر صفحه آزمون پذیرش است: در CI، nginx جای CDN را می‌گیرد، ولی CDN واقعی را باید دستی بررسی کرد.

## کدام سمت پشت CDN است

CDN جلوی سمتِ **شنونده** می‌نشیند و سمت وصل‌شونده به یک edge از CDN وصل می‌شود.

| حالت | وصل می‌شود | پشت CDN | CDN معمول |
|---|---|---|---|
| `direct` | entry (ایران) | exit (خارج) | Cloudflare |
| `reverse` | exit (خارج) | entry (ایران) | CDN با edge داخل ایران، مثل ArvanCloud |

## پیکربندی

حالت direct از راه Cloudflare، با exit که `wss` را به Cloudflare سرو می‌کند (حالت SSL/TLS برابر **Full (strict)**):

```toml
# entry (Iran): dials a Cloudflare edge
role = "entry"
mode = "direct"

[[forward]]
listen = "0.0.0.0:443"
target = "127.0.0.1:443"

[tunnel]
transport = "wss"
remote = "104.16.0.1:443"          # a clean Cloudflare IP (or tunnel.example.com:443)
token = "..."

[tunnel.ws]
path = "/api/v1/stream"
host = "tunnel.example.com"        # also used as the TLS server name
early_data = true                  # hello inside the upgrade request: one round trip less
```

```toml
# exit (abroad): the origin behind Cloudflare
role = "exit"
mode = "direct"

[tunnel]
transport = "wss"
listen = "0.0.0.0:443"
token = "..."

[tunnel.ws]
path = "/api/v1/stream"
host = "tunnel.example.com"        # optional: reject requests for other hosts

[tunnel.tls]
cert = "/etc/kariz/origin.pem"     # Cloudflare Origin CA or Let's Encrypt certificate
key = "/etc/kariz/origin.key"
```

جایگزین: exit به‌جای آن `ws` ساده سرو کند (مثلاً روی پورت ۸۰ یا ۸۰۸۰) و Cloudflare حالت SSL/TLS برابر **Flexible** را بگیرد. entry باز هم به edge با `wss` وصل می‌شود. پای Cloudflare تا origin آن‌وقت TLS نیست، ولی رمزنگاری خودِ تانل باز هم ترافیک را می‌پوشاند.

نکته‌ها:

- **به‌سمت CDN از `tls.pin_sha256` استفاده نکنید.** entry گواهی edge از CDN را می‌بیند که عمومی معتبر است و زود‌به‌زود عوض می‌شود. راستی‌آزمایی پیش‌فرض (ریشه‌های Mozilla) آنجا درست است. pin برای وصل شدن مستقیم به سرور خودتان با گواهی خودامضاست.
- **mux روشن می‌ماند** (پیش‌فرض `ws` / `wss`). هر `keepalive` (پیش‌فرض ۳۰ ثانیه) ping می‌زند، که WebSocket را از بریده شدن به‌عنوان بی‌کار نجات می‌دهد: Cloudflare بی‌کارها را حدود ۱۰۰ ثانیه بعد می‌برد. اگر `tuning.keepalive_secs` بالای ۹۰ باشد `kariz check` هشدار می‌دهد. بدون mux، یک اتصال کاربرِ ساکت را CDN می‌برد.
- **پورت‌هایی که Cloudflare پروکسی می‌کند:** HTTPS ‏443، 2053، 2083، 2087، 2096، 8443؛ HTTP ‏80، 8080، 8880، 2052، 2082، 2086، 2095.
- probeهایی که از راه CDN به origin می‌رسند برای هر مسیری جز `ws.path` صفحهٔ `404` nginx را می‌گیرند، و برای upgradeِ روی `ws.path` که early dataاش hello معتبر ندارد هم.

## فهرست دستی (Cloudflare)

راه‌اندازی:

۱. DNS: یک رکورد `A` برای `tunnel.example.com` ← IP خروجی، **Proxied** (ابر نارنجی).
۲. SSL/TLS ← Overview: برای origin از نوع `wss` حالت **Full (strict)** (برای `ws` حالت **Flexible**).
۳. Network: **WebSockets** روشن (پیش‌فرض).
۴. پورت origin یکی از پورت‌هایی است که Cloudflare پروکسی می‌کند (فهرست بالا).
۵. اختیاری: فایروال origin فقط بازه‌های IP از Cloudflare را بپذیرد.
۶. `kariz check -c <config>` روی هر دو سمت: `config OK`، بدون هشدار.

بررسی‌ها (هرکدام را تیک بزنید و تاریخ و commit کاریز را بنویسید):

- [ ] `curl -sI https://tunnel.example.com/` ← `404`، `server: cloudflare`.
- [ ] `curl -s https://tunnel.example.com/api/v1/stream` (بدون upgrade) ← `404`.
- [ ] اول exit را راه بیندازید، بعد entry را. exit به تعداد `mux.connections` یک‌بار `mux session from the entry side established` لاگ می‌کند و entry هیچ reconnectی لاگ نمی‌کند.
- [ ] با `RUST_LOG=kariz=debug` روی exit: `websocket upgrade request accepted` مقدار `client=<entry IP>` (از `CF-Connecting-IP`) و `early=` بالای ۰ را نشان می‌دهد.
- [ ] ترافیک از پورت forward کار می‌کند: یک دانلود بزرگ، به‌علاوهٔ اتصال‌های کوتاهِ موازی زیاد (مثلاً یک مرورگر از راه پروکسی‌ای که روی exit سرو می‌شود).
- [ ] بی‌کاری: یک اتصال کاربر باز کنید، تانل را ۵ دقیقه بی‌کار بگذارید، بعد دوباره از همان اتصال استفاده کنید. باز هم کار می‌کند و reconnectی لاگ نمی‌شود.
- [ ] exit را دوباره راه بیندازید: entry ظرف چند ثانیه دوباره وصل می‌شود و اتصال‌های تازه کار می‌کنند.
- [ ] با `mux.max_lifetime_secs = 600`: sessionها هر ~۱۰ دقیقه جایگزین می‌شوند و اتصال‌های باز نمی‌شکنند.
- [ ] `ws.path` غلط روی entry: `websocket upgrade refused with HTTP 404` لاگ می‌کند.

عیب‌یابی:

| نشانه | علت محتمل |
|---|---|
| Entry: `upgrade refused with HTTP 404` | `ws.path` / `ws.host` فرق دارند، یا (با early data) hello رد شده: ناهمخوانی token، ساعت‌های ناهماهنگ؛ لاگ exit را ببینید. |
| Entry: HTTP ‏`521` / `522` | origin خاموش است، یا پورت از Cloudflare در دسترس نیست. |
| Entry: HTTP ‏`525` / `526` | Full / Full (strict): origin TLS سرو نمی‌کند، یا گواهی‌اش برای آن نام معتبر نیست. |
| Entry: خطای گواهی | pin به‌سمت CDN (`pin_sha256` را بردارید)، یا `tls.sni` / `ws.host` نام پروکسی‌شده نیست. |
| قطع شدن تقریباً هر ۱۰۰ ثانیه | mux خاموش است، یا `keepalive_secs` بالای ~۹۰ است. |

## ArvanCloud (حالت reverse)

برای CDNهایی با edge داخل ایران همان تنظیم‌ها به‌کار می‌رود، با نقش‌های جابه‌جا: entry (ایران) origin پشت CDN است و exit به edge از CDN وصل می‌شود. پشتیبانی WebSocket و timeout بی‌کاری CDN را بررسی کنید. فهرست بالا با جابه‌جا شدن «entry» و «exit» صدق می‌کند. *هنوز راستی‌آزمایی نشده؛ نتیجه‌ها اینجا اضافه می‌شوند.*

## نتیجه‌ها

| تاریخ | CDN | حالت | ترنسپورت (edge / origin) | Early data | نتیجه | یادداشت |
|---|---|---|---|---|---|---|
| | Cloudflare | direct | wss / wss | روشن | | |
| | Cloudflare | direct | wss / ws | روشن | | |
| | ArvanCloud | reverse | wss / wss | روشن | | |
