import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";
import { parseESCodeBuiltinModelConfigRules } from "@escode/provider";

describe("Zhipu standard API templates", () => {
  it.each(["escode-builtin.json", "escode-builtin.test.json"])(
    "%s 声明标准计费端点、Key 获取入口和 GLM-5.3 系列媒体能力",
    async (filename) => {
      const release = JSON.parse(
        await readFile(new URL(`../../../config/provider/${filename}`, import.meta.url), "utf8"),
      );
      const templates = release.config.providerConfigRules.templateRules;
      for (const template of templates) {
        expect(
          new URL(template.config.access.apiKeyManagementUrl).protocol,
          template.templateId,
        ).toBe("https:");
        expect(template.config.access).not.toHaveProperty("teamApiKeyManagementUrl");
      }
      const rules = parseESCodeBuiltinModelConfigRules(release.config.modelConfigRules);
      for (const family of ["zai", "bigmodel"]) {
        const templateId = `${family}-standard-api`;
        const template = templates.find(
          (item: { templateId: string }) => item.templateId === templateId,
        );
        expect(template, templateId).toBeDefined();
        const label = `${family === "zai" ? "Z.ai" : "BigModel"} API`;
        expect(template.templateNameMap).toEqual({ "zh-CN": label, "en-US": label });
        const config = template.config;
        const host = family === "zai" ? "api.z.ai" : "open.bigmodel.cn";
        expect(config.api).toEqual({
          type: "openai-chat-completions",
          baseUrl: `https://${host}/api/paas/v4`,
        });
        expect(config.access.type).toBe("api-key");
        expect(config.access.apiKeyManagementUrl).toBe(
          family === "zai"
            ? "https://z.ai/manage-apikey/apikey-list"
            : "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
        );
        for (const modelId of ["GLM-5.3", "GLM-5.3-Flash"]) {
          const resolved = rules.resolve({
            templateId,
            providerId: "personal-fixture",
            modelId,
            apiType: config.api.type,
            baseUrl: config.api.baseUrl,
          });
          expect(config.builtinModelIds).toContain(modelId);
          expect(resolved.enabled).toBe(true);
          // 标准 API 走 chat-completions 端点：只有 Flash 带媒体输入，两者都没有网关专属能力。
          expect(resolved.properties?.inputFormat?.supportsImage).toBe(modelId.endsWith("Flash"));
          expect(resolved.properties?.supportsNativeWebSearch).toBe(false);
          expect(resolved.properties?.supportsMidConversationSystem).toBe(false);
        }
      }
    },
  );
});
