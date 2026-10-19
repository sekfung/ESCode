import { GlobalNoticeSurface } from "@/global-notice/GlobalNoticeSurface.js";
import type { GlobalNoticeStore } from "@/global-notice/globalNoticeStore.js";

/** 左下角全局通知槽位；营销触达已随产品关闭移除，槽位只保留 GlobalNoticeSurface。 */
export function SidebarBottomActivity({
  noticeStore,
}: {
  noticeStore?: GlobalNoticeStore | null;
} = {}) {
  return <GlobalNoticeSurface store={noticeStore ?? undefined} />;
}
