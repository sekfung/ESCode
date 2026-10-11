# language: zh-CN
# 规格：docs/circuit/README.md §8、§10.3（ADR 0005：Circuit IR 由 Host 服务持有）
# 运行前提：
# - Desktop 开发版以 ESCODE_DATA_BASE_DIR 指向临时目录隔离用户数据（IR 持久化在应用数据目录 circuits/ 下）。
# - 临时 workspace 内先复制 packages/services/test/fixtures/circuit/rp2040-minimal/ 的板子
#   （RP2040_minimal.kicad_sch / .kicad_pcb）与 firmware/ 里的固件样例；源文件改动直接用测试写盘完成。
# - 第 5、7 条需要一条带 `::circuit{"irId":"…"}` 的 assistant 消息：由 session replay fixture 提供，
#   不调用真实模型（§10.3「E2E 不依赖真实模型」）。
# - 第 7 条用普通 Web 客户端：Web 没有 CircuitService，只验证静态降级卡片。
功能: 电路面板

  场景: 在文件树中打开 .kicad_sch 后渲染并联动 BOM 与网络表
    假如 Desktop 窗口已打开含 "hardware/RP2040_minimal.kicad_sch" 的临时 workspace
    当 用户在文件树中点击 "hardware/RP2040_minimal.kicad_sch"
    那么 右侧面板出现电路面板，并显示图纸（"circuit-schematic"）与 revision "r1"
    并且 面板不显示 stale 提示，侧栏 BOM 列出解析出的器件
    当 用户点击原理图里的器件 "U1"
    那么 该器件在原理图中被高亮，连接 "I2C1_SDA" 的引脚与导线一并高亮
    并且 侧栏网络表中 "I2C1_SDA" 一行被选中，BOM 中 "U1" 一行同步选中

  场景: 面板内修改器件属性后 revision 加 1 且修正记录作者是用户
    假如 电路面板已打开 "hardware/RP2040_minimal.kicad_sch" 且当前为 "r1"
    当 用户在 BOM 中选中 "R1" 并把"值"改为 "4.7k" 后点击"应用修正"
    那么 面板 revision 变为 "r2"，BOM 中 "R1" 显示 "4.7k"
    当 用户切到"修正记录"页签
    那么 列表出现一条 "修改属性" 记录，作者显示"用户"，revision 为 "r2"

  场景: 过期 baseRevision 的写入被拒绝且不覆盖数据
    假如 电路面板已打开 "hardware/RP2040_minimal.kicad_sch"（本地 revision 为 "r1"）
    并且 同一 IR 已被外部写成 "r2"（Agent 或另一个窗口完成一次写入）
    当 用户在面板中把 "R1" 的"值"改为 "10k" 并点击"应用修正"
    那么 面板提示写入被拒绝并建议刷新（"circuit-write-conflict"）
    并且 BOM 中 "R1" 的值仍然是面板刷新前的 "r1" 数据，没有被 "10k" 覆盖
    当 用户点击提示里的"刷新"
    那么 面板 revision 变为 "r2" 并显示外部写入后的值

  场景: 源文件外部变化后重新解析，修正按 UUID 保留，失效的标记出来
    假如 电路面板已打开 "hardware/RP2040_minimal.kicad_sch" 且已有一条针对 "R1" 的修正
    当 用户在外部把源文件里的 "R2" 删除并保存
    那么 面板出现 stale 提示（"circuit-stale-banner"）
    当 用户点击提示里的"重新解析"
    那么 stale 提示消失，revision 加 1
    并且 针对 "R1"（UUID 未变）的修正仍然保留在修正记录里，且未标记失效
    并且 修正记录里出现标记为"已失效"的记录，可供用户删除
    当 用户点击该失效记录的"删除"
    那么 该记录从修正记录列表中消失，其它修正不受影响

  场景: 点击对话中的 ::circuit 卡片打开面板并定位到对应 IR
    假如 对话里有一条 assistant 消息独占一行写着 '::circuit{"irId":"<IR>","title":"RP2040_minimal"}'
    当 用户点击该引用卡片的"打开面板"
    那么 右侧面板打开电路面板，并显示该源文件对应的 IR（"circuit-panel" 的 data-circuit-ir-id 与引用一致）
    并且 面板不因为卡片打开而重复解析：文件已有 IR 时直接读取（revision 保持不变）
    当 用户把同一行引用写在代码块里（```text 围栏内）
    那么 正文保持原样，不渲染引用卡片

  场景: 选中器件插入输入框生成引用 chip 且草稿不发送
    假如 电路面板已打开 "hardware/RP2040_minimal.kicad_sch"
    并且 输入框为空
    当 用户在 BOM 中选中 "U1" 并点击"插入到输入框"
    那么 输入框上方出现引用 chip "U1"，且提示草稿未发送
    当 用户改选网络 "I2C1_SDA" 并点击"插入到输入框"
    那么 输入框上方出现引用 chip "NET:I2C1_SDA"
    并且 对话里没有新增用户消息（草稿没有自动发送）
    当 用户发送任意文本
    那么 发送内容带上电路引用块，用户消息行上显示 "U1" 与 "NET:I2C1_SDA" 两枚 chip

  场景: Web 客户端里的 ::circuit 显示静态降级卡片
    假如 用户使用 Web 客户端打开含该 IR 的 workspace
    并且 对话里有一条带 '::circuit{"irId":"<IR>","title":"RP2040_minimal","componentCount":24,"netCount":41}' 的 assistant 消息
    当 消息渲染完成
    那么 显示静态卡片：标题 "RP2040_minimal"、器件数 "24"、网络数 "41"、"仅 Desktop 可用"
    并且 卡片没有"打开面板"入口，也不加载电路面板、不订阅 circuit.changed 事件
