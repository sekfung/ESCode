import { BUILTIN_MODEL_PROVIDER_IDS, type AppSettings } from "@escode/shared";
import { describe, expect, it, vi } from "vitest";
import { ensureProviderFamilyDomainMigration } from "@/lib/providerFamilyDomainMigration.js";

function createServices(params: {
  settings?: Partial<AppSettings>;
  selectableProviderIds?: string[];
}) {
  const update = vi.fn(async (_patch: Partial<AppSettings>) => {});
  return {
    settingService: {
      get: vi.fn(async () => ({
        recentProjects: [],
        locale: "zh-CN" as const,
        modelProviderFamilyModes: {},
        providerFamilyDomainMigrated: false,
        ...params.settings,
      })),
      update,
    },
    modelSelectionService: {
      getView: vi.fn(async () => ({
        revision: 1,
        providers: (params.selectableProviderIds ?? []).map((providerId) => ({
          providerId,
          config: {
            access: { type: "api-key" as const, apiKey: "test-key" },
          },
          models: [],
        })),
      })),
    },
    update,
  };
}

describe("ensureProviderFamilyDomainMigration", () => {
  it("从唯一可选的 family Provider 推断 providerFamilyDomain", async () => {
    const services = createServices({
      selectableProviderIds: [BUILTIN_MODEL_PROVIDER_IDS.zaiStartPlan],
    });

    await ensureProviderFamilyDomainMigration(services);

    expect(services.update).toHaveBeenCalledWith(
      expect.objectContaining({
        providerFamilyDomain: "zai",
        providerFamilyDomainMigrated: true,
      }),
    );
  });

  it("存在多个可选 family 时只标记已迁移，不猜测 domain", async () => {
    const services = createServices({
      selectableProviderIds: [
        BUILTIN_MODEL_PROVIDER_IDS.zaiStartPlan,
        BUILTIN_MODEL_PROVIDER_IDS.bigmodelStartPlan,
      ],
    });

    await ensureProviderFamilyDomainMigration(services);

    expect(services.update).toHaveBeenCalledWith(
      expect.not.objectContaining({
        providerFamilyDomain: expect.any(String),
      }),
    );
    expect(services.update).toHaveBeenCalledWith(
      expect.objectContaining({
        providerFamilyDomainMigrated: true,
      }),
    );
  });

  it("provider 缓存未恢复时暂不标记已迁移", async () => {
    const services = createServices({
      selectableProviderIds: [],
    });

    await ensureProviderFamilyDomainMigration(services);

    expect(services.update).not.toHaveBeenCalled();
  });

  it("已有 providerFamilyDomain 时不重复迁移", async () => {
    const services = createServices({
      settings: {
        providerFamilyDomain: "bigmodel",
        providerFamilyDomainMigrated: true,
      },
    });

    await ensureProviderFamilyDomainMigration(services);

    expect(services.update).not.toHaveBeenCalled();
  });
});
