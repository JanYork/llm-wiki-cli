(() => {
  "use strict";

  const localeKey = "lwc-site-locale";

  function storeLocale(locale) {
    try {
      localStorage.setItem(localeKey, locale);
    } catch {
      // Navigation still works when storage is blocked.
    }
  }

  try {
    if (document.documentElement.lang === "en" && localStorage.getItem(localeKey) === "zh-CN") {
      window.location.replace("zh-CN/");
      return;
    }
  } catch {
    // Keep the requested URL when storage is blocked.
  }

  for (const link of document.querySelectorAll("[data-locale]")) {
    link.addEventListener("click", () => storeLocale(link.dataset.locale));
  }

  function selectPrompt(element) {
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(element);
    selection.removeAllRanges();
    selection.addRange(range);
  }

  for (const button of document.querySelectorAll("[data-copy-target]")) {
    button.addEventListener("click", async () => {
      const prompt = document.getElementById(button.dataset.copyTarget);
      const status = document.getElementById(button.dataset.copyStatus);
      const original = button.dataset.copyLabel;

      try {
        if (!navigator.clipboard?.writeText) throw new Error("clipboard unavailable");
        await navigator.clipboard.writeText(prompt.textContent);
        button.textContent = button.dataset.copySuccess;
        status.textContent = button.dataset.copySuccess;
        window.setTimeout(() => {
          button.textContent = original;
        }, 2200);
      } catch {
        selectPrompt(prompt);
        status.textContent = button.dataset.copyFailure;
      }
    });
  }
})();
