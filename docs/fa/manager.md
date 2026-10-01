# اسکریپت مدیر

[English](../manager.md) · **فارسی**

`scripts/kariz.sh` کاریز و [پنل وب](panel.md) آن را روی یک سرور لینوکس با systemd نصب می‌کند و یک سرور را به پنل وصل می‌کند. بعد از اولین اجرا، دستور `kariz-manager` هم هست.

**تانل‌ها اینجا ساخته نمی‌شوند.** سرورها، تانل‌ها، شبکه‌های خصوصی، پشتیبان‌گیری و به‌روزرسانی سرورهای دیگر همه در پنل وب انجام می‌شوند. مدیر فقط راهی است که کاریز را روی سرور بگذارد و پنل را راه بیندازد؛ از آن به بعد در مرورگر کار می‌کنید.

## نصب

با دسترسی root، روی هر سرور:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh)
```

بار اول روی یک سرور، هستهٔ کاریز را خودش نصب می‌کند و بعد می‌پرسد این سرور برای چه کاری است: فقط هسته (تانل‌ها از پنلی روی سرور دیگر ساخته می‌شوند)، پنل وب هم همین‌جا، یا وصل شدن به پنلی که هست. از آن به بعد هر اجرا یک منو باز می‌کند که بالایش نام سرور و آدرس‌های IPv4 و IPv6 آن است:

| | |
|---|---|
| **۱** نصب یا به‌روزرسانی کاریز | CPU شما را پیدا می‌کند (x86_64، aarch64 یا armv7)، آخرین ریلیز را می‌گیرد، SHA-256 و امضایش را می‌سنجد و `kariz`، `kariz-panel`، unitهای systemd و دستور `kariz-manager` را نصب می‌کند |
| **۲** پنل وب و agent | پنل را روی این سرور نصب کنید، لینک ورود بسازید، رمز بگذارید، یا این سرور را با کد پیوستن به یک پنل وصل کنید |
| **۳** حذف کاریز | آنچه نصب کرده را متوقف و پاک می‌کند |

اگر GitHub دانلودهایتان را محدود می‌کند (سرورهای زیاد پشت یک آدرس)، پیش از اجرای اسکریپت `GITHUB_TOKEN` را روی هر personal access token بگذارید: فقط برای برداشتن همین محدودیت استفاده می‌شود.

راه‌های دیگر نصب:

```bash
kariz-manager install --version v1.0.0        # a specific release
kariz-manager install --binary ./kariz        # a binary you built yourself
```

## راه معمول

۱. روی سروری که پنل را نگه می‌دارد: یک‌خطی را اجرا کنید، **۲** و بعد **install** را بزنید. آدرس پنل و یک لینک ورود یک‌بارمصرف چاپ می‌شود.
۲. لینک را در مرورگر باز کنید و وارد شوید. از اینجا به بعد در پنل کار کنید: [کار با پنل](using-the-panel.md).
۳. روی هر سرور دیگر: یک‌خطی را اجرا کنید، **۲** و بعد **agent** را بزنید و کد پیوستنی را که پنل داده بچسبانید (سرورها، افزودن سرور). آن سرور حالا در پنل دیده می‌شود.
۴. در پنل، بین هر دو سرورش تانل بسازید.

## دستورها

منو و دستورها کار یکسانی می‌کنند:

```bash
kariz-manager install [--version vX.Y.Z] [--binary PATH]   # Kariz (and the panel program)
kariz-manager update [--version vX.Y.Z]                    # a new release; what runs is restarted
kariz-manager uninstall [--yes]                            # --yes also deletes the configs

kariz-manager panel install [--port N] [--host H]   # the panel on this server: address, certificate, a login link
kariz-manager panel link [--host H]                 # another one-time login link
kariz-manager panel password [--stdin]              # a new admin password
kariz-manager panel status | logs                   # the service, its address / follow the log
kariz-manager panel uninstall [--yes]

kariz-manager --agent kz1_... [--yes]               # connect this server to a panel (the code is from the panel)
kariz-manager agent status | logs | remove
kariz-manager status                                # what runs here and this server's addresses
```

سروری که agent دارد را می‌شود با یک کد تازه به پنل دیگری وصل کرد: اسکریپت همین را می‌گوید، می‌پرسد (`--yes` پرسش را رد می‌کند)، agent قبلی را برمی‌دارد و agent تازه را جایش راه می‌اندازد. پنل قبلی تا وقتی آنجا حذفش نکنید این سرور را آفلاین نشان می‌دهد.

## به‌روزرسانی و حذف

`install` و `update` **امضای** ریلیز را می‌سنجند (از ۰٫۱۱ ریلیزی که امضای معتبر نداشته باشد نصب نمی‌شود؛ ریلیزهای قدیمی‌تر امضا ندارند و با یک هشدار پذیرفته می‌شوند) و SHA-256 آن را. `update` هر چه در حال اجراست دوباره راه می‌اندازد: همهٔ تانل‌ها، پنل و agent، تا برنامهٔ جدید را به کار ببرند. وقتی سروری به پنل وصل شد، پنل می‌تواند آن را (و خودش را) با یک دکمه به‌روز کند: [به‌روزرسانی از پنل](panel.md#بهروزرسانی).

`update` خودِ اسکریپت مدیر را عوض نمی‌کند. برای نسخهٔ تازهٔ مدیر دوباره بگیریدش:

```bash
curl -fsSL -o /usr/local/bin/kariz-manager \
    https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh
chmod +x /usr/local/bin/kariz-manager
```

`uninstall` کاریز را کامل از سرور برمی‌دارد: همهٔ تانل‌ها، agent (سرویس، هویت و لینک‌های شبکهٔ خصوصی‌ای که ساخته)، پنل وب اگر باشد، و برنامه‌ها. فهرست آنچه پاک می‌شود را می‌گوید و یک بار می‌پرسد، بعد می‌پرسد کانفیگ تانل‌ها و داده‌های پنل (tokenها، سرورها، گواهی) هم برود یا نه؛ `--yes` به همه جواب بله می‌دهد. سروری که به پنل وصل بوده تا وقتی در پنل حذفش نکنید آنجا آفلاین دیده می‌شود. `agent remove` فقط agent را برمی‌دارد.

## لاگ‌ها

لاگ‌های کاریز با اولویت systemd برای هر خط در journal می‌روند. پنل آن‌ها را در صفحهٔ لاگ نشان می‌دهد؛ روی سرور:

```bash
journalctl -u kariz@main -f                 # follow one tunnel
journalctl -u kariz@main -p warning         # only warnings and errors
journalctl -u kariz-panel -f                # the panel
journalctl -u kariz-agent -f                # the agent
```

## خوب است بدانید

- root، systemd و `curl` یا `wget` لازم دارد.
- تانل‌ها کانفیگ‌های معمولی کاریزند در `/etc/kariz/<name>.toml` که سرویس `kariz@<name>` اجرایشان می‌کند؛ agent پنل می‌نویسدشان. می‌توانید بخوانیدشان و [مرجع پیکربندی](configuration.md) هر تنظیم را توضیح می‌دهد، ولی تغییر را در پنل بدهید تا دو سمت هم‌قدم بمانند.
- unit سرویس `systemd/kariz@.service` است.
