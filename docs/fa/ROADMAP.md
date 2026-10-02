# طراحی و پروتکل

[English](../ROADMAP.md) · **فارسی**

## لایه‌ها

```
[ Port forward / UDP forward ]      user-facing listeners on the entry side
[ Session: stream + datagram ]      each TCP connection is a stream, each UDP flow a datagram flow
[ Mux (tcpmux) ]                    many sessions over few long-lived connections
[ Crypto: TLS 1.3 / AEAD + PSK ]
[ Transport ]  tcp | ws | wss | quic | kcp | udp
```

ترنسپورت‌ها دو گروه‌اند:

- **stream (قابل‌اعتماد):** tcp، tcpmux، ws، wss، quic، kcp
- **datagram (بی‌ضمانت):** udp، datagramهای QUIC

**UDP روی هر ترنسپورت:** روی ترنسپورت‌های stream، بسته‌های UDP قاب‌بندی می‌شوند (شناسهٔ جریان + طول) تا UDP جایی که خودِ UDP مسدود است هم کار کند. روی ترنسپورت‌های datagram همان‌طور که هستند فرستاده می‌شوند، برای کمترین تأخیر. UDP روی TCP/WS از head-of-line blocking رنج می‌برد، پس برای بازی datagramهای QUIC، KCP یا UDP خام ترجیح داده می‌شوند.

هر دو حالت **reverse** (خروجی به ورودی وصل می‌شود) و **direct** (ورودی به خروجی وصل می‌شود) برای هر ترنسپورت پشتیبانی می‌شوند.

## پروتکل (نسخهٔ ۲)

۰. **ترنسپورت**: یک اتصال TCP (`tcp`، `tcpmux`)، یک WebSocket روی TCP (`ws`) یا TLS (`wss`) که شبیه مرورگر است، یا KCP روی UDP که هر بسته‌اش با کلیدی از توکن مهر می‌شود (`kcp`) (به `src/transport/` نگاه کنید). هر چیز زیر داخل آن می‌رود. (`quic` لایه‌های زیر را با لایه‌های خودش جایگزین می‌کند.)
۱. **Handshake** (به `src/crypto/handshake.rs` نگاه کنید): احراز هویت دوطرفه با توکن مشترک به‌علاوهٔ تبادل X25519 برای forward secrecy. hello پوشیده و با طولی تصادفی padding می‌شود، پس هیچ بایت ثابت و اندازهٔ ثابتی ندارد؛ replayها رد می‌شوند.
۲. **Recordها** (به `src/crypto/record.rs` نگاه کنید): هر چیز بعد از handshake به‌صورت recordهای AEAD فرستاده می‌شود (ChaCha20-Poly1305 یا AES-256-GCM، طول‌ها هم رمز شده‌اند)، با یک کلید برای هر جهت. `encryption = "none"` handshake را نگه می‌دارد ولی recordها را رد می‌کند.
۳. **Mux** (اختیاری، به `src/mux/` نگاه کنید): streamهای زیاد روی یک اتصال، با کنترل جریان برای هر stream، pingها و چرخش ملایم. برای `tcpmux`، `ws`، `wss` به‌طور پیش‌فرض روشن است.
۴. **Open** (به `src/proto.rs` نگاه کنید): ورودی `kind | len | target` می‌فرستد، خروجی مقصد را dial می‌کند. بدون mux، خروجی با یک بایت وضعیت جواب می‌دهد؛ با mux، ورودی داده را فوراً می‌فرستد و dialِ ناموفق stream را reset می‌کند.
۵. **Relay**: کپی دوطرفه از میان لایهٔ record.

بدون mux، در حالت reverse، handshake وقتی رخ می‌دهد که خروجی pool را پر می‌کند، پس باز کردن یک اتصال کاربر یک رفت‌وبرگشت به‌علاوهٔ dial مقصد هزینه دارد؛ در حالت direct، درخواست open به‌صورت early data با 0-RTT درست پشت hello فرستاده می‌شود (و با `ws.early_data`، داخل درخواست upgrade در WebSocket). با mux، handshake یک‌بار برای هر session رخ می‌دهد و باز کردن یک stream هیچ رفت‌وبرگشتی اضافه نمی‌کند.

اتصالی که handshake را رد شود فوراً بسته نمی‌شود بلکه ۵ تا ۳۰ ثانیهٔ تصادفی تخلیه می‌شود. قالب سیم نسخهٔ ۲ با v0.1 سازگار نیست: هر دو سمت را با هم ارتقا دهید. استتار فراتر از این (شکل‌دهی padding و زمان‌بندی، fallback برای active-probe) برنامه‌ریزی نشده.
