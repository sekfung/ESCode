import type { ESCodeProvider } from "@escode/shared";

export const ESCODE_MODE_OPTION_LABEL_IDS: Record<ESCodeProvider, Record<string, string>> = {
  glm: {
    build: "mode.label.glm.build",
    edit: "mode.label.glm.edit",
    plan: "mode.label.glm.plan",
    yolo: "mode.label.glm.yolo",
  },
};

export const ESCODE_MODE_OPTION_DESCRIPTION_IDS: Record<ESCodeProvider, Record<string, string>> = {
  glm: {
    build: "mode.description.glm.build",
    edit: "mode.description.glm.edit",
    plan: "mode.description.glm.plan",
    yolo: "mode.description.glm.yolo",
  },
};
