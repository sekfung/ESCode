import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import type { PluginSandboxRegisterInput } from "@escode/shared/mcp-apps";
import type { GenUiStateChange, GenUiStateTarget } from "@escode/shared/gen-ui";
import { IGenUiService } from "./contract.js";
import { exampleGenUiPrepare } from "./contract.example.js";
import { createGenUiService } from "./node.js";

/**
 * contract 的消费方式：用 node 入口装配真实服务（状态根与输出根注入临时目录），逐条走契约声明的方法，
 * 保证调用方按声明使用即可拿到文档、受限沙箱与按会话作用域的状态；读取细节、存储并发与锁由 adapters 测试覆盖。
 */
async function createFixture() {
  const stateRoot = await mkdtemp(join(tmpdir(), "gen-ui-contract-state-"));
  const outputRoot = await mkdtemp(join(tmpdir(), "gen-ui-contract-output-"));
  const registrations: PluginSandboxRegisterInput[] = [];
  const service = createGenUiService({
    stateRoot,
    outputRoot,
    registerSandbox: async (input) => {
      registrations.push(input);
      return {
        instance: input.instance,
        sandboxId: "sandbox",
        initId: 1,
        partition: "gen-ui",
        shellUrl: "escode-sandbox://fixture",
      };
    },
  });
  return {
    service,
    registrations,
    outputRoot,
    async dispose() {
      service.dispose();
      await rm(stateRoot, { recursive: true, force: true });
      await rm(outputRoot, { recursive: true, force: true });
    },
  };
}

it("频道名与模块声明一致", () => {
  expect(IGenUiService.channelName).toBe("gen-ui");
});

it("契约示例可准备宿主沙箱：只收片段、受限 CSP、内联初始状态", async () => {
  const fixture = await createFixture();
  try {
    const target: GenUiStateTarget = {
      workspacePath: exampleGenUiPrepare.workspacePath,
      sessionId: exampleGenUiPrepare.sessionId,
      path: exampleGenUiPrepare.path,
    };
    await fixture.service.setState({
      target,
      state: { modelContent: "上一轮摘要", privateContent: null },
    });
    const handle = await fixture.service.prepareSandbox(exampleGenUiPrepare);
    const [input] = fixture.registrations;
    expect(handle.partition).toBe("gen-ui");
    expect(input?.contentKind).toBe("gen-ui");
    expect(input?.csp).toMatchObject({
      connectDomains: [],
      frameDomains: [],
      baseUriDomains: [],
    });
    // 初始状态先于运行时脚本，widget 首帧不用等 IPC。
    expect(input?.html).toContain("上一轮摘要");
    expect(input!.html.indexOf("escode-gen-ui-initial-state")).toBeLessThan(
      input!.html.indexOf("__ESCODE_GEN_UI_RUNTIME__"),
    );
    await expect(
      fixture.service.prepareSandbox({
        ...exampleGenUiPrepare,
        html: "<html><body>document</body></html>",
      }),
    ).rejects.toThrow(/fragment/u);
  } finally {
    await fixture.dispose();
  }
});

it("状态按 workspace identity + session 作用域读写，并投影持久状态的变化", async () => {
  const fixture = await createFixture();
  try {
    const scope = {
      workspacePath: "/workspace",
      workspaceIdentity: "remote:fixture",
      sessionId: "s1",
    };
    const target: GenUiStateTarget = { ...scope, path: "/workspace/chart.html" };
    const changes: GenUiStateChange[] = [];
    const subscription = fixture.service.onStateChanged((change) => changes.push(change));
    await fixture.service.setState({
      target,
      state: { modelContent: { value: 1 }, privateContent: null },
    });
    expect(await fixture.service.getState(target)).toEqual({
      modelContent: { value: 1 },
      privateContent: null,
    });
    expect(await fixture.service.listState(scope)).toEqual([
      { path: target.path, state: { modelContent: { value: 1 }, privateContent: null } },
    ]);
    // 同一路径在不同会话下互不可见。
    expect(await fixture.service.listState({ ...scope, sessionId: "s2" })).toEqual([]);
    await vi.waitFor(() =>
      expect(changes.at(-1)).toMatchObject({ target, state: { modelContent: { value: 1 } } }),
    );
    subscription.dispose();
  } finally {
    await fixture.dispose();
  }
});

it("readDocument 只读输出根内的片段，越界与整文档都拒绝", async () => {
  const fixture = await createFixture();
  try {
    const inside = join(fixture.outputRoot, "chart.html");
    await writeFile(inside, '<div id="chart">ok</div>');
    const target: GenUiStateTarget = {
      workspacePath: fixture.outputRoot,
      sessionId: "s1",
      path: inside,
    };
    expect(await fixture.service.readDocument(target)).toEqual({
      path: inside,
      html: '<div id="chart">ok</div>',
    });
    await expect(
      fixture.service.readDocument({
        ...target,
        path: join(fixture.outputRoot, "..", "escape.html"),
      }),
    ).rejects.toThrow(/outside the host output directory/u);
    const documentPath = join(fixture.outputRoot, "full.html");
    await writeFile(documentPath, "<html><body>document</body></html>");
    await expect(fixture.service.readDocument({ ...target, path: documentPath })).rejects.toThrow(
      /fragment/u,
    );
  } finally {
    await fixture.dispose();
  }
});
