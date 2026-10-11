import { useESCodeStoreWithDefault } from "@/store/StoreProvider.js";

export function useIsOfficeMode(): boolean {
  return useESCodeStoreWithDefault((state) => state.interfaceMode === "office", false);
}
