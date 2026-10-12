declare module "yazl" {
  import { EventEmitter } from "node:events";
  import type { Readable } from "node:stream";

  type LazyReadStreamCallback = (error: Error | null, stream?: Readable) => void;

  /** 与 yazl 真实 API 一致：第三个参数用于固定 mtime 等（插件安装测试按内容比对 zip）。 */
  interface AddBufferOptions {
    mtime?: Date;
    mode?: number;
    compress?: boolean;
  }

  export class ZipFile extends EventEmitter {
    readonly outputStream: Readable;

    addFile(realPath: string, metadataPath: string): void;
    addBuffer(buffer: Buffer, metadataPath: string, options?: AddBufferOptions): void;
    addReadStreamLazy(
      metadataPath: string,
      getReadStreamFunction: (callback: LazyReadStreamCallback) => void,
    ): void;
    end(): void;
  }
}
