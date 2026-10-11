import type { UiLocale, SupportedLocale } from "@escode/contracts";
import { enUS } from "./locales/en-US.js";
import { zhCN } from "./locales/zh-CN.js";
import {
  DEFAULT_LOCALE,
  detectLocale,
  isSupportedLocale,
  isUiLocale,
  resolveLocale,
  SUPPORTED_LOCALES,
} from "./locale.js";
import type { ESCodeCopy } from "./types.js";

export {
  DEFAULT_LOCALE,
  SUPPORTED_LOCALES,
  detectLocale,
  isSupportedLocale,
  isUiLocale,
  resolveLocale,
};
export type { LocaleDetectionInput } from "./locale.js";
export type { CliCopy, TuiCopy, UiLocale, SupportedLocale, ESCodeCopy } from "./types.js";

const CATALOGS: Record<SupportedLocale, ESCodeCopy> = {
  "en-US": enUS,
  "zh-CN": zhCN,
};

export function getESCodeCopy(locale?: UiLocale | string, detected?: string | null): ESCodeCopy {
  return CATALOGS[resolveLocale(locale, detected)];
}
