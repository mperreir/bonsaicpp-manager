import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import fr from "./fr.json";
import en from "./en.json";

/// Supported UI locales. To add a language: copy `fr.json`,
/// translate the values (keys unchanged), import it here and
/// register it in `resources` + `supportedLngs` + `LOCALES`.
/// `npm run i18n:check` fails when a language misses keys.
export type AppLocale = "fr" | "en";

export const LOCALES: { id: AppLocale; label: string }[] = [
  { id: "fr", label: "Français" },
  { id: "en", label: "English" },
];

const STORAGE_KEY = "bonsai.locale";

export function resolveInitialLocale(): AppLocale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "fr" || saved === "en") return saved;
  } catch {
    /* ignore */
  }
  // Default: the system language (English OS -> English, else French).
  if (typeof navigator !== "undefined" && navigator.language.toLowerCase().startsWith("en")) {
    return "en";
  }
  return "fr";
}

export function persistLocale(lng: AppLocale) {
  try {
    localStorage.setItem(STORAGE_KEY, lng);
  } catch {
    /* ignore */
  }
}

export function syncDocumentLang(lng: string) {
  document.documentElement.lang = lng.startsWith("en") ? "en" : "fr";
}

void i18n
  .use(initReactI18next)
  .init({
    resources: {
      fr: { translation: fr },
      en: { translation: en },
    },
    lng: resolveInitialLocale(),
    fallbackLng: "fr",
    supportedLngs: ["fr", "en"],
    interpolation: { escapeValue: false },
  });

syncDocumentLang(i18n.language);
i18n.on("languageChanged", syncDocumentLang);

export default i18n;
