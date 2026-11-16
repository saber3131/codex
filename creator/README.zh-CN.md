# 造物主 Creator 内核

造物主 Creator 的编程与电脑操控内核。它由开源的 OpenAI Codex CLI（Apache-2.0）改造而来，Codex 能做的它都能做：读写代码、运行命令、调用 MCP 插件、技能（skills）、子代理、沙箱和审批。每次发布都会先合并当天最新的开源 Codex。

和原版 Codex 的区别：

- 用**紫微星通行证**登录，不需要 ChatGPT 账号，所以 ChatGPT 额度用完也不会被锁住。
- 模型走**龍華樹**（b31.si），费用从您通行证账号的龍華树 API 余额里扣。
- 可以选择 **B31.Si-1.3 · 万能许愿机**：由指挥官模型拆分任务，按华林园排行榜把每一部分交给该领域第一的模型，最后汇总成一份结果。
- 数据单独放在 `C:\Users\您的用户名\.creator`，不会读取或改动 Codex 的 `.codex` 文件夹。

## 第一次使用（Windows）

1. 解压后，在文件夹空白处按住 Shift 点右键，选「在终端中打开」。
2. 输入 `.\creator.exe login`，按提示填写通行证邮箱和密码。
   - 用 Google 或 GitHub 注册、没有设置密码的账号，改用 `.\creator.exe login --code`，系统会把验证码发到邮箱。
3. 登录成功后输入 `.\creator.exe`，就能像 Codex 一样直接对话、让它改代码或操作电脑。

## 常用命令

| 命令 | 作用 |
| --- | --- |
| `creator` | 打开对话界面 |
| `creator login` / `creator logout` | 登录 / 退出紫微星通行证 |
| `creator status` | 查看账号、龍華樹余额和 B31.Si 分工 |
| `creator sync` | 按最新排行榜更新模型目录和 B31.Si 分工 |
| `creator exec "任务"` | 不打开界面，直接执行一个任务 |
| `creator app-server` | 给造物主桌面版用的后台接口（与 Codex 桌面版同一协议） |

在对话界面里输入 `/model` 可以换模型，默认是 B31.Si-1.3。

## 文件夹里有什么

- `config.toml`：设置（模型、龍華树接口、B31.Si 专家角色）。
- `models.json`：模型目录，由 `creator sync` 生成。
- `b31.json`、`agents\*.toml`：B31.Si-1.3 的分工表。
- `passport.json`：登录信息（龍華树 Key），不要发给别人。

## 许可证

基于 OpenAI Codex（Apache License 2.0），保留原 LICENSE 和 NOTICE。造物主改动见 `creator/MODIFICATIONS.md`。本软件不是 OpenAI 官方产品。
