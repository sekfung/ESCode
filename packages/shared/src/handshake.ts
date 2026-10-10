export interface HelloMessage {
  type: "escode-hello";
  version: string;
  platform: string;
  arch: string;
  pid: number;
}

export interface HelloAckMessage {
  type: "escode-hello-ack";
  version: string;
  clientId: string;
}
