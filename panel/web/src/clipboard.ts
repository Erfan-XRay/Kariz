/**
 * Copies text to the clipboard; true if it went. The Clipboard API where the page may use it
 * (a secure context: https, localhost), else the old way (a hidden field and `execCommand`),
 * which also works on a page opened over plain http and in older browsers.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Refused (no permission, the page is not focused): try the old way.
  }
  const back = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const field = document.createElement("textarea");
  field.value = text;
  field.setAttribute("readonly", "");
  field.setAttribute("aria-hidden", "true");
  // Out of sight and out of the way; 16px so that iOS does not zoom the page to the field.
  field.style.cssText = "position:fixed;top:0;inset-inline-start:0;width:1px;height:1px;opacity:0;pointer-events:none;font-size:16px";
  document.body.appendChild(field);
  let ok = false;
  try {
    field.select();
    field.setSelectionRange(0, text.length);
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  field.remove();
  back?.focus({ preventScroll: true });
  return ok;
}
