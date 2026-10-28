// 构建期把 zod 的语言包裁剪为仅 en（见 tsup.config.ts 的 resolve 插件）。
//
// zod v4 的 `core/index`、`classic/external`、`mini/external` 都以命名空间方式
// `export * as locales`，导致即使调用方从不使用 `z.locales`，全部 ~60 个语言包也会
// 进入 main 静态图（约 267 KB）。应用没有调用 `z.config(z.locales.xx())`，
// zod 默认错误信息就是 en；因此保留 en 即可，其余语言按需再引入。
export { default as en } from "zod/v4/locales/en.js";
