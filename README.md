# RE0Drive

个人自托管的 RE0 资源搜索与 115 分享转存。多架构 Docker 镜像开箱即用，适合 NanoPi 一类 **arm64 / 4GB NAS**（无需在 NAS 上编译）。

镜像：`ghcr.io/blantonsbourbon/re0drive`（与仓库一样默认**私有**）。协议与开发细节见 [`docs/`](docs/)。

---

## NAS 快速上手

### 前提

- Docker + Compose v2（`docker compose version`）
- 能访问 GitHub / ghcr.io（若 SSH 22 被墙，用下方 **443** 方式）
- GitHub 账号对私有仓库有权限；准备一个 **classic PAT**，至少勾选 `read:packages`（用 curl 拉脚本时再加 `repo`）

### 一键部署（推荐）

在 NAS 上任选一种方式拿到并执行 `scripts/deploy.sh`（脚本会写 `compose.yaml` / `.env`，**不会**写入任何密钥）：

**方式 A — HTTPS 443 拉脚本（私有仓库需 PAT）**

```sh
export GH_USER=blantonsbourbon          # 你的 GitHub 用户名
export GH_TOKEN=ghp_xxxx                # classic PAT：read:packages + repo
# 可选：安装目录 / 镜像标签（默认 latest）
# export RE0DRIVE_DIR=$HOME/re0drive
# export RE0DRIVE_TAG=0.1.0

curl -fsSL -H "Authorization: Bearer $GH_TOKEN" \
  "https://raw.githubusercontent.com/blantonsbourbon/re0drive/main/scripts/deploy.sh" \
  | sh -s --
```

**方式 B — Git 经 SSH 443 克隆后部署**（适合已配好 deploy key / SSH key 的环境）

```sh
mkdir -p ~/.ssh
# 让 git@github.com 走 443，绕过被墙的 22
printf 'Host github.com\n  HostName ssh.github.com\n  Port 443\n  User git\n' >> ~/.ssh/config
chmod 600 ~/.ssh/config

git clone git@github.com:blantonsbourbon/re0drive.git
cd re0drive
./scripts/deploy.sh                  # 或：RE0DRIVE_TAG=0.1.0 ./scripts/deploy.sh
```

脚本会：创建目录 → 若缺失则写入 `compose.yaml` / `.env`（默认 `RE0DRIVE_HOST_BIND=0.0.0.0`）→ 提示 `docker login ghcr.io` → `docker compose pull && up -d` → 等待 `/health` → 打印访问地址。重复执行即**更新**（数据在命名卷中保留）。

固定版本示例：

```sh
RE0DRIVE_TAG=0.1.0 ./scripts/deploy.sh
# 或编辑 .env：RE0DRIVE_IMAGE=ghcr.io/blantonsbourbon/re0drive:0.1.0
```

### 首次配置顺序

浏览器打开 `http://<NAS局域网IP>:8787`（仅本机访问时把 `.env` 的 `RE0DRIVE_HOST_BIND` 改回 `127.0.0.1` 后重新 `./scripts/deploy.sh`）：

1. **创建管理员**（强密码，只此一次）
2. **导入 / 填写中转签名密钥**，模式切到「中转 API」并保存（导入步骤见 [`docs/development.md`](docs/development.md)）
3. **115 扫码登录**（设置 → 115 网盘）
4. **RE0 授权**（设置 → 授权 / 扫码授权 RE0）
5. **机器令牌**（设置 → 机器令牌 → 生成；只显示一次，交给 Hermes）
6. （可选）**Telegram**：Bot Token + 白名单 chat/user id

### 更新 / 回滚 / 备份

```sh
# 更新到 latest（或 .env 里钉死的标签）
cd ~/re0drive && ./scripts/deploy.sh

# 回滚到某版本
RE0DRIVE_TAG=0.1.0 ./scripts/deploy.sh

# 备份：必须同时保留 re0drive.db 与 master.key（停服后复制，或用 SQLite 在线备份）
docker compose stop
docker compose cp re0drive:/data ./backup-data
docker compose start
```

拥有 `re0drive.db` + `master.key` 的人可以解密配置，请妥善保管备份。

### 环境变量（`.env`）

| 变量 | 默认 | 用途 |
|---|---|---|
| `RE0DRIVE_IMAGE` | `ghcr.io/blantonsbourbon/re0drive:latest` | 镜像（可用 `RE0DRIVE_TAG` 由脚本改写） |
| `RE0DRIVE_HOST_BIND` | `0.0.0.0`（脚本生成的 `.env`） | 宿主机绑定；`127.0.0.1` 仅本机 |
| `RE0DRIVE_COOKIE_SECURE` | `0` | 经 HTTPS 反代时设 `1`，并转发 `X-Forwarded-Proto: https` |
| `RE0_GATEWAY_URL` | `https://re0.tgtodrive.top` | **仅新数据卷**初始化时的中转地址 |
| `RE0_GATEWAY_SECRET` | 空 | **仅新数据卷**初始化时的 HMAC 密钥；已有数据请在网页改 |
| `TZ` | `Asia/Hong_Kong` | 签到时区 |

容器内还支持：`RE0DRIVE_BIND`（镜像默认 `0.0.0.0:8787`）、`RE0DRIVE_DATA`（`/data`）、`RUST_LOG`。测试用覆盖见 `.env.example`。

### 排障

| 现象 | 处理 |
|---|---|
| `pull access denied` / 401 | `docker login ghcr.io -u <用户>`，密码用只含 `read:packages` 的 PAT；确认有该私有 Package 权限 |
| 登录提示 IP 异常 / 会话异常 | 经 HTTPS 反代时设 `RE0DRIVE_COOKIE_SECURE=1`；或检查反代是否改写了 Host / 未转发 Cookie |
| 115 Cookie 失效 | 设置页扫码重登；可开 Cookie 保活与 Telegram 告警 |
| 端口被占 / 访问不到 | `ss -ltnp \| grep 8787`；确认 `RE0DRIVE_HOST_BIND` 与防火墙；局域网用 `0.0.0.0` |
| SSH `git clone` 超时 | 用上文 `ssh.github.com:443` 的 `~/.ssh/config`，或改走方式 A 的 curl + PAT |

---

## Hermes / 机器调用 API

机器令牌不依赖浏览器 Cookie：

1. 网页 → 连接与设置 → **机器令牌** → 生成（`r0d_` + 64 hex，**只显示一次**；服务端只存 SHA-256）。`POST/GET/DELETE /api/machine-token` 需会话 + `X-Re0drive-Request: 1`。
2. 请求头 `Authorization: Bearer <token>`。带了 `Authorization` 就只校验令牌；Bearer 请求不需要 CSRF 头。
3. 令牌仅可访问：`POST /api/resources/search`、`GET|POST /api/jobs`、`GET /api/jobs/{id}`。

```sh
export RE0DRIVE=http://127.0.0.1:8787 TOKEN=r0d_...   # 勿写入仓库或日志

curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"tmdb_id":550,"media_type":"movie"}' $RE0DRIVE/api/resources/search

curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"resource":{...搜索结果中的一项...},"target_cid":""}' $RE0DRIVE/api/jobs

curl -s -H "Authorization: Bearer $TOKEN" $RE0DRIVE/api/jobs/<job_id>
```

- 搜索：`tmdb_id` / 纯数字 query 只查该 ID；片名合并 TMDB 前 4 条。中转空结果 → `"resources":[]`（不用演示数据填充）。
- 每项含 `slug`、`title`、`pan_type`、`selectable`（仅 115 为 true）、`share_size` / `video_resolution` / `source` / `subtitle_*`（多为数组）、`unlock_points`、`is_unlocked`、`remark`。
- 任务：`target_cid` 可省略（= 根目录 `0`）；`max_points` 可省略（= 该版本标价，解锁前重查）。成功后 `received[]` 含 `share_id` / `local_id` / `parent_cid` / `name` / `size`（本 App 不移动文件）。

---

## 功能摘要

片名或 TMDB ID 搜索；完整版本字段；非 115 列出但不可转存；订阅只列候选、从不自动解锁；115 扫码 + Cookie 保活；RE0 授权/签到；Telegram 长轮询机器人；机器令牌供 Hermes。凭据加密存储、接口不回显。

更多：[`docs/development.md`](docs/development.md) · [`docs/protocol.md`](docs/protocol.md) · [`docs/verification.md`](docs/verification.md)
