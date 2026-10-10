# 开发、导入密钥与联调说明

本文承接 README 的 NAS 快速上手；日常部署不必读完。

## 本机从源码运行

Rust 1.85.1 或更新版本：

```sh
cargo build --release --locked
./target/release/re0drive
```

浏览器访问 `http://127.0.0.1:8787`，首次打开创建管理员。默认数据目录为当前目录的 `data/`。

若使用提供的 Linux x86_64 二进制，直接解压执行 `./re0drive`，无需安装 Rust。该包在 Debian 13 上构建，所需最高 glibc 符号为 2.34；其他系统建议用 Docker 或自行编译。

只读诊断：`./re0drive --check-gateway`（或 `./target/release/re0drive --check-gateway`）。检查授权状态，不解锁、不签到、不转存。

本地从源码构建镜像：取消 `compose.yaml` 中 `build: .` 的注释后执行 `docker compose up -d --build`。

## 中转密钥：从本地 TgtoDrive 导入

中转服务除浏览器 OAuth 外，还校验客户端 HMAC 签名。本项目不在源码或下载包内置原客户端签名密钥。可用自己中转提供的密钥，或从本地 TgtoDrive 客户端导入。

**停止 RE0Drive**，用与 TgtoDrive 匹配的 Python 运行导入工具：

```sh
python3 scripts/import-tgtodrive.py /app/hdhive_user_client.so \
  --output /path/to/re0drive/data/bootstrap.json
```

可选 `--install-file /path/to/re0_install.json` 保留原 `install_id` 与授权；不指定则使用新安装标识，随后重新授权。导入工具不打印密钥，禁止网络和子进程，只读本地客户端。`bootstrap.json` 以 0600 创建，下次启动加密写入数据库并删除。已保存的 115 Cookie 与 TMDB Key 会保留。

已验证的 TgtoDrive 8.7.16 原生模块需 **同镜像的 CPython 3.13.15**。原生模块依赖严格 ABI，不要用其他补丁版本的 Python 加载。导入是一次性步骤，RE0Drive 运行不依赖 Python 或原客户端。

Docker 场景可在 TgtoDrive 容器内跑脚本（挂载脚本与输出目录），再把文件拷到 RE0Drive 卷并重启：

```sh
docker compose stop re0drive
docker compose cp ./bootstrap.json re0drive:/data/bootstrap.json
docker compose start re0drive
```

容器用户 UID 为 `10001`，导入文件需可读。宿主机生成后可 `chown 10001:10001 bootstrap.json`，保留 0600；成功导入后删除宿主机明文副本。

也可在设置页直接填写签名密钥，或首次启动时设置 `RE0_GATEWAY_SECRET`。

协议细节见 [protocol.md](protocol.md)。

## 连接账号（细节）

### 扫码登录

- **115**：设置页 → 115 网盘 → 扫码登录。流程：`qrcodeapi.115.com` 取令牌 → 轮询 `/get/status/` → `/app/1.0/{端}/1.0/login/qrcode/` 换 Cookie。Cookie 在服务端加密保存。**同一登录端同时只能在线一个**，会挤掉该端已有登录。
- **RE0**：中转无原生扫码；“扫码授权 RE0”把签名的 `/auth/start` 渲成二维码，手机完成 re0.me 授权后页面轮询 `/api/token/status`。需先切到中转 API 模式并保存签名密钥。二维码约 10 分钟过期。

### 手动配置要点

目标目录 ID 为 `0` 时接收到根目录。Cookie 失效时在设置中更新或扫码重登。更换中转地址、安装标识、签名密钥、115 Cookie 或连接模式后，旧队列不会沿用新账号上下文，请重新创建任务。

## 功能边界与任务状态

- 交互搜索**不按积分上限过滤**；订阅只列候选、**从不自动解锁**（含 0 积分与旧 `auto_transfer`）。
- 接收成功后按文件名匹配本地 fid 写入 `job.received`；本 App 不移动文件（由 Hermes 整理）。
- 本地积分上限只是解锁前的预检查；中转解锁 body 只有 `slug`。费用未知/负数/超上限则跳过；已拥有资源仍请求链接。
- 状态：`queued → checking → unlocking → transferring → success`；`failed` 可重试；`needs_review` 需人工核对后重试。
- ED2K 只列出（`selectable:false`）；整季/单集语义、多账号自动切换不在本版范围。

## 验证

```sh
cargo test --locked
cargo build --release --locked && tests/smoke.sh
```

`tests/smoke.sh` 自带 mock（RE0 / 115 / TMDB / Telegram）与代理绊线，不触达真实第三方。详见 [verification.md](verification.md)。

镜像标签由 `.github/workflows/docker.yml` 在推送 `main` / `v*` / `workflow_dispatch` 时构建：`latest`、`sha-<7位>`，以及 `v1.2.3` → `1.2.3` / `1.2`（`v0.x` 不生成裸主版本号标签）。
