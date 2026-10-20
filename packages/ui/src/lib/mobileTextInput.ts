import { useEffect, useState } from "react";

const MOBILE_TEXT_INPUT_QUERY = "(max-width: 767px) and (hover: none) and (pointer: coarse)";
const MOBILE_COMPACT_VIEWPORT_QUERY = "(max-width: 767px)";

function getMediaQueryMatches(query: string): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia(query).matches
  );
}

function subscribeMediaQuery(query: string, listener: (matches: boolean) => void): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    listener(false);
    return () => {};
  }

  const mediaQueryList = window.matchMedia(query);
  const update = () => listener(mediaQueryList.matches);
  update();

  // Bugfix: 部分移动端 WebView / 旧 Safari 仍只支持 addListener。
  // 这里沿用远控 shell 的兼容方式，避免手机端输入框挂载时因监听 API 不存在而崩溃。
  if (typeof mediaQueryList.addEventListener === "function") {
    mediaQueryList.addEventListener("change", update);
    return () => mediaQueryList.removeEventListener("change", update);
  }

  mediaQueryList.addListener(update);
  return () => mediaQueryList.removeListener(update);
}

function getIsMobileTextInputViewport(): boolean {
  return getMediaQueryMatches(MOBILE_TEXT_INPUT_QUERY);
}

function getIsMobileCompactViewport(): boolean {
  return getMediaQueryMatches(MOBILE_COMPACT_VIEWPORT_QUERY);
}

export function subscribeMobileTextInputViewport(listener: (matches: boolean) => void): () => void {
  return subscribeMediaQuery(MOBILE_TEXT_INPUT_QUERY, listener);
}

export function subscribeMobileCompactViewport(listener: (matches: boolean) => void): () => void {
  return subscribeMediaQuery(MOBILE_COMPACT_VIEWPORT_QUERY, listener);
}

export function useIsMobileTextInputViewport(): boolean {
  const [isMobileTextInputViewport, setIsMobileTextInputViewport] = useState(
    getIsMobileTextInputViewport,
  );

  useEffect(() => subscribeMobileTextInputViewport(setIsMobileTextInputViewport), []);

  return isMobileTextInputViewport;
}

export function useIsMobileCompactViewport(): boolean {
  const [isMobileCompactViewport, setIsMobileCompactViewport] = useState(
    getIsMobileCompactViewport,
  );

  useEffect(() => subscribeMobileCompactViewport(setIsMobileCompactViewport), []);

  return isMobileCompactViewport;
}

export function resolveChatEnterSubmits({
  isMobileTextInputViewport,
  preferEnterNewline,
}: {
  isMobileTextInputViewport: boolean;
  preferEnterNewline: boolean;
}): boolean {
  return !(preferEnterNewline && isMobileTextInputViewport);
}

export function resolveChatEnterShortcut({
  enterSubmits,
}: {
  enterSubmits: boolean;
}): "Enter" | undefined {
  return enterSubmits ? "Enter" : undefined;
}
