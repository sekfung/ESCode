/**
 * usb MCP server（stdio）。由官方插件宿主（`__escode-plugin-host`）按 manifest 的 `mcpServers` 启动。
 *
 * 与 serial 插件的差别：没有 broker、没有反向请求——抓包与解码都在本进程内完成，产物是文件。
 * 因此这里不校验会话上下文，也不依赖 Host；退出时唯一要保证的是杀掉还在跑的抓包子进程。
 */
import { Server } from "@modelcontextprotocol/server";
import { type UsbToolContext } from "./tools.js";
export type UsbBaseContext = Omit<UsbToolContext, "signal">;
/** 工作区与插件数据目录由 manifest 注入；缺失时退到 cwd 与临时目录，保证独立运行时也能用。 */
export declare function resolveUsbBaseContext(env: Record<string, string | undefined>): UsbBaseContext;
export declare function createUsbMcpServer(base: UsbBaseContext): Server;
/** 由插件宿主导入并调用。进程结束前必须杀掉在跑的抓包工具，避免留下孤儿进程与 .partial 文件。 */
export declare function main(): Promise<void>;
//# sourceMappingURL=server.d.ts.map