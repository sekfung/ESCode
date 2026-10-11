import { getESCodeCopy, type SupportedLocale, type UiLocale } from "@escode/i18n";

export function formatCliHelp(
  version: string,
  locale?: UiLocale,
  detectedLocale?: SupportedLocale,
): string {
  return getESCodeCopy(locale, detectedLocale).cli.help(version);
}
