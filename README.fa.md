<div dir="rtl">

# computer-use

یک قابلیت **استفاده از کامپیوتر (computer use)** به سبک Codex برای ایجنت‌ها، نوشته‌شده با Rust. این پروژه همان کاری را برای ایجنت شما می‌کند که قابلیت computer use اپ Codex انجام می‌دهد: دیدن و کار کردن با اپ‌های گرافیکی واقعی — کلیک، تایپ، خواندن وضعیت روی صفحه و انجام کارهای چنداپلیکیشنی — ساخته‌شده روی **API دسترس‌پذیری (accessibility)** بومی هر سیستم‌عامل به‌همراه **اسکرین‌شات**، و با **تأیید جداگانه برای هر اپ**.

این یک ماژول مستقل است: می‌توانید آن را به‌صورت یک **سرور MCP** اجرا کنید (با Codex، Claude Code یا هر ایجنت سازگار با MCP کار می‌کند)، یا **کتابخانه** آن را داخل ایجنت خودتان استفاده کنید.

> English guide: [README.md](README.md).

## چرا معماری‌اش مثل Codex است؟

computer use در Codex «اول دسترس‌پذیری» است: درخت accessibility اپ را می‌خواند، روی عناصر به‌صورت معنایی عمل می‌کند و فقط وقتی مجبور شود سراغ پیکسل می‌رود. این پروژه دقیقاً همین معماری و رفتار را دارد:

- **درخت accessibility + اسکرین‌شات.** ابزار `get_app_state` یک درخت accessibility هرس‌شده و **شماره‌دار** به‌همراه یک اسکرین‌شات از پنجره برمی‌گرداند. هر اکشن روی یک `element_index` انجام می‌شود و از اکشن بومی سیستم (فشردن، تنظیم مقدار، تغییر وضعیت و…) استفاده می‌کند؛ برای همین حتی روی پنجره‌های پس‌زمینه هم کار می‌کند.
- **شماره‌ها فقط تا نوبت بعد معتبرند.** ایندکس عناصر فقط تا فراخوانی بعدیِ `get_app_state` معتبر است؛ فراخوانی‌های بعدی یک **diff** برمی‌گردانند (مگر اینکه `disable_diff` بدهید) تا مصرف توکن پایین بماند.
- **همان ده ابزاری** که پلاگین Computer Use در Codex دارد — `list_apps`، `get_app_state`، `click`، `perform_secondary_action`، `set_value`، `select_text`، `scroll`، `drag`، `press_key`، `type_text` — به‌علاوهٔ `launch_app`.
- **تأیید و ایمنی.** هر اپ قبل از کنترل‌شدن باید تأیید شود (یک‌بار / برای این نشست / همیشه)، و ترمینال‌ها، پرامپت‌های رمز و امنیت سیستم‌عامل، و خودِ اپ میزبانِ ایجنت هرگز قابل کنترل نیستند.

## ابزارها

| ابزار | کارش چیست |
|------|-----------|
| `list_apps` | فهرست اپ‌های در حال اجرا (شناسه، pid، وضعیت پنجره). |
| `launch_app` | اجرای یک اپ با نام/شناسهٔ بسته/فایل اجرایی و انتظار تا باز شدن پنجره. |
| `get_app_state` | درخت accessibility شماره‌دارِ پنجره **+ یک اسکرین‌شات**. اول هر نوبت صدا بزنید. |
| `click` | کلیک روی یک عنصر با `element_index` (از اکشن بومی‌اش استفاده می‌کند) یا روی مختصات `x`/`y` بر حسب پیکسلِ اسکرین‌شات. |
| `perform_secondary_action` | اکشن غیرکلیکیِ فهرست‌شده برای عنصر (`show_menu`, `increment`, `expand`, `toggle`…). |
| `set_value` | تنظیم مستقیم متن یک فیلد، اسلایدر، یا چک‌باکس/سوییچ. |
| `select_text` | انتخاب یک زیررشته (یا کل متن) در یک عنصر متنی. |
| `scroll` | اسکرول یک عنصر یا ناحیهٔ زیر یک نقطه. |
| `drag` | کشیدن بین عناصر یا نقاط. |
| `press_key` | یک کلید یا میان‌بر، مثل `cmd+s`، `ctrl+shift+t`، `Down Down Return`. |
| `type_text` | تایپ در عنصری که فوکوس دارد. |

قرارداد کامل کاری که مدل باید دنبال کند در [`skill/SKILL.md`](skill/SKILL.md) است.

## معماری

- **موتور (engine)** مستقل از سکو است و همهٔ رفتاری را دارد که باید همه‌جا یکسان باشد: هرس‌کردن درخت خام accessibility به یک نمای شماره‌دارِ کم‌حجم، تخصیص ایندکس پایدار برای هر نوبت، گرفتن diff بین اسنپ‌شات‌ها، نگاشت پیکسل اسکرین‌شات به مختصات صفحه، و اعمال سیاست تأیید.
- هر **backend** یک آداپتور نازک روی یک سیستم‌عامل است:

| | macOS | Windows | Linux |
|---|---|---|---|
| درخت و اکشن‌ها | Accessibility (AX) API | UI Automation | AT-SPI2 روی D-Bus |
| ورودی | `CGEvent` که به pid مقصد فرستاده می‌شود (پس‌زمینه، نشانگر تکان نمی‌خورد) | `SendInput` | XTest |
| تصویربرداری | `CGWindowListCreateImage` | `PrintWindow` | X11 `GetImage` |

## اجرا به‌صورت سرور MCP

</div>

```bash
cargo build --release -p computer-use-mcp
# باینری در target/release/computer-use-mcp
```

<div dir="rtl">

**Claude Code** (در فایل `.mcp.json`):

</div>

```json
{
  "mcpServers": {
    "computer-use": {
      "command": "/path/to/target/release/computer-use-mcp",
      "args": ["serve"]
    }
  }
}
```

<div dir="rtl">

**Codex** (در `~/.codex/config.toml`):

</div>

```toml
[mcp_servers.computer-use]
command = "/path/to/target/release/computer-use-mcp"
args = ["serve"]
```

<div dir="rtl">

سرور با JSON-RPC 2.0 روی stdio صحبت می‌کند و `initialize`، `tools/list`، `tools/call` و تأیید هر اپ از طریق `elicitation/create` را پیاده کرده است.

### خط فرمان (CLI)

</div>

```bash
computer-use-mcp doctor        # سکو، مجوزها، مسیر پیکربندی
computer-use-mcp apps          # فهرست اپ‌های در حال اجرا
computer-use-mcp state "TextEdit" --screenshot shot.png
computer-use-mcp call click '{"app":"TextEdit","element_index":3}'
```

<div dir="rtl">

## استفاده به‌صورت کتابخانه

</div>

```rust
use computer_use::{AllowApprover, tools};

let mut engine = computer_use::platform_engine()?;
let defs = tools::definitions();  // این‌ها را به مدل خود بدهید
let out = engine.call_tool("list_apps", serde_json::json!({}), &mut AllowApprover);
println!("{}", out.text);
```

<div dir="rtl">

برای یک نشست کامل و قابل‌اجرا روی بک‌اند ماک (روی هر سیستمی اجرا می‌شود):

</div>

```bash
cargo run -p computer-use --example mock_session
```

<div dir="rtl">

## آماده‌سازی هر سکو

- **macOS** — به اپ میزبان مجوز **Accessibility** و **Screen Recording** بدهید. ورودی به فرایند مقصد فرستاده می‌شود، پس نشانگر کاربر تکان نمی‌خورد.
- **Windows** — برای UI Automation مجوز خاصی لازم نیست؛ ورودی مختصاتی با `SendInput` روی دسکتاپ فعال انجام می‌شود.
- **Linux** — به یک باس accessibility از نوع AT-SPI2 و یک نمایشگر X11 نیاز دارد. برای ابزارک‌تان accessibility را روشن کنید (مثلاً GTK وقتی باس a11y موجود باشد پل at-spi را بار می‌کند). Wayland برای ورودی/تصویربرداریِ مصنوعی پشتیبانی نمی‌شود؛ از نشست X11 (یا XWayland) استفاده کنید.

## ایمنی

ترمینال‌ها، مدیرهای رمز، و پرامپت‌های احراز هویت/رضایتِ سیستم‌عامل به‌طور سخت مسدودند و هرگز خودکار نمی‌شوند، به‌همراه خودِ اپ ایجنت. اولین استفاده از هر اپ دیگر با تأیید کنترل می‌شود. به مدل گفته شده (در فایل skill) که پیش از اکشن‌های حساس مثل ارسال، خرید یا حذف مکث کند.

## مجوز

تحت [MIT](LICENSE-MIT) یا [Apache-2.0](LICENSE-APACHE) به انتخاب شما.

</div>
