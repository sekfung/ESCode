import type { CompatibilityManifest } from "./source-server/compatibility.js";

export interface HelloMessage {
  type: "escode-hello";
  version: string;
  platform: string;
  arch: string;
  pid: number;
  compatibility?: CompatibilityManifest;
}

export interface HelloAckMessage {
  type: "escode-hello-ack";
  version: string;
  clientId: string;
}
