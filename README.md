# RE0Drive

个人自托管的 RE0 订阅、资源搜索和 115 分享转存 App。使用 TgtoDrive 的中转协议，默认服务为 `https://re0.tgtodrive.top`，不依赖申请 RE0 官方开发者 API。

后端采用 Rust + Axum + Tokio，SQLite 保存订阅和队列；网页无需 Node 构建，随单个可执行文件发布。HTTP 连接复用，中转查询最多 4 个并发，写入外部服务的任务串行处理。语言和框架没有通用的“性能最好”，这里优先降低常驻开销，外部服务的响应速度与限流仍是主要约束。

## 功能

- 片名搜索（配置 TMDB Key），或直接使用 `tmdb:693134` / 数字 ID 搜索。
- **NullBR 回退**：中转返回空时，用公开 NullBR API（`GET /{movie|tv}/{tmdb}/115`，文档见 [nullbr.readthedocs.io](https://nullbr.readthedocs.io/)）按 TMDB ID 查询；结果标记 `origin=nullbr`，115 分享可选中转存（无需 RE0 解锁），非 115 为 `selectable:false`。两边都空则返回空列表。App ID / API Key 自行配置，加密保存。
- **115 Cookie 保活**：定期只读探测；失效时写活动日志并（可选）Telegram 告警，到设置页扫码重登。Cookie 始终加密、不回显。
- **Telegram 机器人**：长轮询（无需 Webhook）。白名单 chat/user 可搜索、点按钮确认转存、签到、查 Cookie/任务；Token 加密保存。
- 搜索结果保留中转返回的全部版本信息：`pan_type`、`share_size`、`video_resolution`/`source`/`subtitle_language`/`subtitle_type`（数组原样）、积分、`remark`。非 115 资源（夸克、百度、阿里、迅雷、189、139、ED2K…）照常列出，但 `selectable:false`、没有转存按钮。交互搜索**不按积分上限过滤**。
- 电影、剧集订阅：定时检查并把匹配的 115 版本记为“候选”，**从不自动解锁/转存**（包括 0 积分资源和旧的 `auto_transfer=true` 订阅），由人或 Hermes 选定版本后再提交任务。订阅默认积分上限 20，`auto_transfer` 默认关闭且已不生效。
- 接收成功后列出目标目录，按文件名匹配出**本地 fid**（目录为本地 cid）写入任务结果，供 Hermes 自行移动整理；本 App 不移动文件。
- RE0 浏览器授权、授权状态查询、每日正常签到。
- 查询最新积分 → 解锁分享 → 115 顶层文件/目录分页读取 → 接收分享。
- 同一账号、资源和目标目录去重。成功任务保留；未发生解锁的“已跳过”任务可在后续匹配时重新排队。
- 重启保留任务；外部写入中断或结果不明确的任务进入“待核对”，不会自动重试。
- 管理员密码使用 Argon2，凭据用 ChaCha20-Poly1305 加密保存，接口不回显密钥、Cookie 或 OAuth Token。

默认首次启动为演示模式，示例任务会明确标记，成功统计不包含演示。导入中转密钥后会切换到真实中转模式。只有完成 RE0 授权并设置自己的 115 Cookie，才具备真实转存所需的账号上下文。

## 本机运行

Rust 1.85.1 或更新版本：

```sh
cargo build --release --locked
./target/release/re0drive
```

浏览器访问 `http://127.0.0.1:8787`，首次打开创建管理员。默认数据目录为当前目录的 `data/`。

如果使用提供的 Linux x86_64 二进制，直接解压执行 `./re0drive`，无需安装 Rust。此包在 Debian 13 上构建，所需的最高 glibc 符号版本为 2.34；其他系统建议使用 Docker 或自行编译。

## 中转密钥：从本地 TgtoDrive 导入

中转服务除了浏览器 OAuth 授权，还验证客户端 HMAC 签名。本项目不在源码或下载包里内置原客户端的签名密钥。可以使用自己的中转服务提供的密钥，或从本地 TgtoDrive 客户端导入。

**停止 RE0Drive**，用 TgtoDrive 自身匹配的 Python 运行导入工具。例如在已安装 TgtoDrive 的环境中：

```sh
python3 scripts/import-tgtodrive.py /app/hdhive_user_client.so \
  --output /path/to/re0drive/data/bootstrap.json
```

可选 `--install-file /path/to/re0_install.json` 保留原来的 `install_id` 和关联授权；不指定会使用 RE0Drive 的新安装标识，随后重新授权。导入工具不打印密钥，禁止网络和子进程，只读取本地客户端。`bootstrap.json` 以 0600 权限创建，下次启动时加密写入数据库并删除。已保存的 115 Cookie 和 TMDB Key会保留。

已验证的 TgtoDrive 8.7.16 原生模块需 **同镜像的 CPython 3.13.15**。原生模块依赖严格的 Python ABI，不要用系统里其他补丁版本的 Python 加载。导入是一次性步骤，RE0Drive 运行不依赖 Python 或原客户端。

Docker 场景可在 TgtoDrive 容器内运行该脚本（把脚本与输出目录挂载进去），把生成的文件复制到 RE0Drive 卷中，再重启：

```sh
docker compose stop re0drive
docker compose cp ./bootstrap.json re0drive:/data/bootstrap.json
docker compose start re0drive
```

容器运行用户 UID 为 `10001`，导入文件需要允许该用户读取。示例：宿主机生成文件后执行 `chown 10001:10001 bootstrap.json`，保留 0600 权限；成功导入后删除宿主机剩余的明文副本。

也可在设置页直接填写签名密钥，或者首次启动时设置 `RE0_GATEWAY_SECRET`；无需等待官方 API 申请。

## 连接账号

### 扫码登录（推荐）

- **115**：“连接与设置 → 115 网盘 → 扫码登录 115”。选择登录端（默认支付宝小程序端），用 115 App 扫码并确认。流程与 p115client / TgtoDrive 相同：`qrcodeapi.115.com` 获取令牌 → 轮询 `/get/status/`（等待 / 已扫码 / 已确认 / 已过期 / 已取消）→ `/app/1.0/{端}/1.0/login/qrcode/` 换取 Cookie。Cookie 直接在服务器端加密保存，不经过浏览器。**同一登录端同时只能在线一个**，会挤掉该端已有的登录，建议选不常用的端。
- **RE0**：中转服务没有原生扫码接口，其授权入口是签名的 `/auth/start` 链接。“扫码授权 RE0”把这个链接渲染成二维码（也可点链接在本机打开），用手机完成 re0.me 授权后，页面每 3 秒查询中转 `/api/token/status`，授权成功自动显示。需要先切到中转 API 模式并保存签名密钥。二维码 10 分钟后过期，可点“刷新二维码”。

### 手动配置

1. 在“连接与设置”保留默认中转地址，保存配置。
2. 点击“授权 RE0”，打开生成的链接，在 RE0 完成 OAuth 授权，回到 App 检查授权状态。
3. 填写目标 115 账号的 Cookie，例如 `UID=…; CID=…; SEID=…; KID=…`。这里只使用 Cookie 分享接收接口，不申请 115 OpenAPI。
4. 目标目录 ID 为 `0` 时接收到根目录。已有目录 ID 可从 115 的目录页面 URL 获取。
5. 配置 TMDB API Key可按片名搜索；不配置时仍可用数字 TMDB ID 查询。
6. 搜索资源并选择版本转存；订阅只列候选版本。

Cookie 失效时需要在设置中更新。更换中转地址、安装标识、签名密钥、115 Cookie 或连接模式后，旧队列不会继续使用新的账号上下文；请重新创建任务。

## Docker / NAS

GitHub Actions（`.github/workflows/docker.yml`）在推送到 `main`、推送 `v*` 标签或手动触发时，分别在原生 amd64 / arm64 runner 上构建并推送多架构镜像 `ghcr.io/blantonsbourbon/re0drive`：

| 标签 | 来源 |
|---|---|
| `latest` | `main` 最新提交 |
| `sha-<7位>` | 每次构建的提交 |
| `1.2.3` / `1.2` / `1` | 推送 `v1.2.3` 标签（`v0.x` 不生成主版本号标签） |

NAS 只需拉取镜像，不需要源码或 Rust 构建环境：

```sh
mkdir re0drive && cd re0drive
# 取得 compose.yaml 和 .env.example（仓库为私有时，从仓库下载这两个文件复制到 NAS）
cp .env.example .env            # 按需编辑；不要把 .env 提交到仓库
docker login ghcr.io -u <GitHub 用户名>   # 密码处粘贴只含 read:packages 权限的 PAT（classic）
docker compose pull && docker compose up -d
# 更新：docker compose pull && docker compose up -d   （数据在命名卷中保留）
```

镜像包默认是**私有**的（与仓库一致），NAS 必须先 `docker login ghcr.io`，使用只勾选 `read:packages` 的 Personal Access Token（classic）；凭据保存在 NAS 的 `~/.docker/config.json`。如果不介意公开镜像，也可以在 GitHub → Packages → re0drive → Package settings → Change visibility 设为 Public，之后无需登录即可拉取（镜像内不含任何密钥或 Cookie）。想固定版本时在 `.env` 设置 `RE0DRIVE_IMAGE=ghcr.io/blantonsbourbon/re0drive:sha-xxxxxxx`。本地从源码构建：取消 `compose.yaml` 中 `build: .` 的注释后执行 `docker compose up -d --build`。

默认端口仅绑定 `127.0.0.1:8787`。在 NAS 局域网直接访问时，设置 `.env` 的 `RE0DRIVE_HOST_BIND=0.0.0.0` 后重建容器；通过 HTTPS 反向代理访问时设置 `RE0DRIVE_COOKIE_SECURE=1`，代理转发 `X-Forwarded-Proto: https`。先在自己的受控访问入口创建管理员。

使用命名卷持久化数据。若改为宿主机目录挂载，需要为 UID `10001` 提供该目录的读写权限。数据备份须同时保留 `re0drive.db` 和 `master.key`；服务停止后再复制，或者使用 SQLite 在线备份 API，不要只复制运行中的主 DB 文件。拥有数据库和主密钥的主机用户可以解密配置。

环境变量：

| 变量 | 默认值 | 用途 |
|---|---|---|
| `RE0DRIVE_BIND` | `127.0.0.1:8787` | 服务监听地址；镜像内是 `0.0.0.0:8787` |
| `RE0DRIVE_DATA` | `data` | SQLite 与主密钥目录 |
| `RE0_GATEWAY_SECRET` | 空 | 新数据目录初始化时的 HMAC 密钥 |
| `RE0_GATEWAY_URL` | `https://re0.tgtodrive.top` | 新数据目录初始化时的中转地址 |
| `RE0_INSTALL_ID` | 随机生成 | 新数据目录初始化时的安装标识 |
| `RE0DRIVE_COOKIE_SECURE` | `0` | 强制设置 HTTPS 会话 Cookie |
| `RE0DRIVE_115_BASE_URL` | `https://webapi.115.com` | 115 分享接口地址；仅用于离线测试或自建代理 |
| `RE0DRIVE_115_QR_BASE_URL` | `https://qrcodeapi.115.com` | 115 扫码登录接口地址；仅用于离线测试 |
| `RE0DRIVE_TMDB_BASE_URL` | `https://api.themoviedb.org` | TMDB 接口地址；仅用于离线测试或自建代理 |
| `TZ` | 镜像为 `Asia/Hong_Kong` | 每日签到使用的本地时区 |
| `RUST_LOG` | `re0drive=info,tower_http=info` | 日志级别 |

只读诊断：`./re0drive --check-gateway`（源码构建路径为 `./target/release/re0drive --check-gateway`）。检查授权状态，不解锁资源、不签到、不转存。

## 积分和任务状态

本地积分上限是**请求解锁之前的预检查**，中转解锁 body 只接收 `slug`，没有服务端原子性的最大积分参数。查询与解锁之间若价格变化，服务器实际计费可能变化。费用未知、负数或超过上限时跳过；已拥有资源仍请求服务器返回链接。

`queued → checking → unlocking → transferring → success`。检查阶段出错为 `failed`，可重试。解锁或接收响应丢失、不完整、服务重启中断写入时为 `needs_review`，需要在 RE0 和 115 核对后再手工重试。停用或删除订阅时，尚未执行的任务会跳过；已开始的外部请求会完成。

当前实现支持 **115 分享链接转存**。ED2K 只列出（`selectable:false`），离线下载需单独确认流程，未实现；整季/单集语义、115 目录整理（由 Hermes 负责移动）和多账号自动切换不在本版范围内；订阅的版本与关键词筛选作用于标题、画质、分辨率、来源、字幕和备注。

## NullBR / Cookie / Telegram 配置

在网页「连接与设置」中填写（全部写入加密配置，密钥类字段不回显）：

| 项 | 说明 |
|---|---|
| NullBR 启用 / 地址 / App ID / API Key | 默认地址 `https://api.nullbr.eu.org`；App ID + API Key 在 [NullBR](https://nullbr.readthedocs.io/) 自行申请。仅当 RE0 中转对某个 TMDB ID 返回空时才会请求 NullBR。 |
| Cookie 保活启用 / 间隔（分钟） | 默认开启、360 分钟。也可点「立即检查」。失效后请扫码重登 115。 |
| Telegram 启用 / Bot Token / 白名单 | Token 来自 @BotFather；白名单为数字 chat/user id（逗号分隔）。进程内长轮询 `getUpdates`，可用环境变量 `RE0DRIVE_TG_BASE_URL` 指向自建 Bot API（测试用）。 |

命令：`/search` `/movie` `/tv` `/checkin` `/cookie` `/jobs`；搜索结果带 inline 按钮，点选后才入队转存（仍遵守「先选版本再解锁」、NullBR 直链跳过解锁）。

## Hermes / 机器调用 API

Hermes 用**机器令牌**调用，不依赖浏览器 Cookie：

1. 登录网页 → 连接与设置 → **机器令牌** → “生成 / 轮换令牌”。令牌形如 `r0d_` + 64 位十六进制，**只显示一次**；服务端只保存 SHA-256 哈希和前 10 位前缀。再次生成会让旧令牌立即失效，“撤销”则删除。（也可用会话调用 `POST /api/machine-token`，需 `X-Re0drive-Request: 1`；`GET` 只返回前缀与使用时间，`DELETE` 撤销。）
2. 请求头 `Authorization: Bearer <token>`。带 Bearer 的请求不需要 `X-Re0drive-Request`；一旦带了 `Authorization` 头就只校验令牌，不回落到 Cookie。
3. 令牌**只能**访问：`POST /api/resources/search`、`GET /api/jobs`、`POST /api/jobs`、`GET /api/jobs/{id}`。设置、订阅、账号、令牌管理等其余接口一律 401。

```sh
export RE0DRIVE=http://127.0.0.1:8787 TOKEN=r0d_...   # 不要把令牌写进仓库或日志

# 只查这一个 TMDB ID（数字 query、"tmdb:550" 或 tmdb_id 字段均可；不会混入片名搜索的其他结果）
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"tmdb_id":550,"media_type":"movie"}' $RE0DRIVE/api/resources/search
# 片名搜索：合并 TMDB 前 4 个结果（需在设置中配置 TMDB Key）
#   -d '{"query":"搏击俱乐部","media_type":"movie"}'

# 选定版本后提交（resource 原样传回搜索结果中的对象；target_cid 可省略 = 115 根目录 0；
# max_points 可省略 = 该版本标价，解锁前会重新查询，实际价格更高则跳过、不解锁）
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"resource":{...搜索结果中的一项...},"target_cid":"","max_points":2}' $RE0DRIVE/api/jobs

# 轮询任务
curl -s -H "Authorization: Bearer $TOKEN" $RE0DRIVE/api/jobs/<job_id>
```

搜索响应：`{"mode","total","resources":[...]}`，每项：

| 字段 | 说明 |
|---|---|
| `slug` | 完整 slug（不截断） |
| `title` | 标题 |
| `pan_type` | 中转原值（`115`、`quark`、`ed2k`…） |
| `provider` | 归一化：含 115 → `"115"`，否则为小写原值 |
| `selectable` | 仅 115 为 `true`；其余（含 ED2K，待单独确认流程）为 `false`，提交会被 400 拒绝 |
| `share_size`、`video_resolution`、`source`、`subtitle_language`、`subtitle_type` | 原样（多为数组） |
| `unlock_points`、`is_unlocked`、`remark` | 积分（未知为 `null`）、是否已拥有、备注 |
| `quality`、`size`、`tmdb_id`、`media_type` | 兼容字段（扁平文本） |

中转返回 `data:[]`/`total:0` 时得到 `"resources":[]`，**不会**用演示数据填充。

任务（`job`）字段：`id`、`status`（`queued/checking/unlocking/transferring/success/failed/skipped/needs_review`）、`message`、`target_cid`、`max_points`、`resource`，以及成功后的 `received`：

```json
"received": [
  {"name":"Movie.2160p.mkv","is_dir":false,"share_id":"111","local_id":"9001","parent_cid":"3482983666605360884","size":62599999999},
  {"name":"Extras","is_dir":true,"share_id":"222","local_id":"9002","parent_cid":"3482983666605360884","size":0}
]
```

`share_id` 是分享内的 id（文件用 snap 的 `fid`，目录用 snap 的 `cid`），`local_id` 是接收后在目标目录里的 id（文件为列表中的 `fid`，目录为其自身 `cid`；不会把文件所在目录的 `cid` 当作文件 id）。按名称匹配，115 尚未完成复制时会短暂重试；未匹配到时 `local_id` 为空，`message` 会写明“本地已定位 m/n 项”。目标目录列表失败不影响接收成功的判定。

## 验证

```sh
cargo test --locked                       # 协议 / 流程 / 扫码 / Hermes / NullBR·Cookie·Telegram（tests/features.rs）
cargo build --release --locked && tests/smoke.sh   # 端到端冒烟（含 tripwire）
```

`tests/smoke.sh` 自带环境：在随机本地端口启动 `tests/mock_upstream.py`（RE0 中转 + 115 + TMDB 替身，用测试密钥校验每个请求的 HMAC）和一个全新数据目录的 RE0Drive，所有外部地址都指向替身；同时把代理设为一个关闭的本地端口作为“绊线”，任何意外的非本机请求都会直接失败，不会离开本机。覆盖：115 / RE0 扫码登录、静态资源与 CSP、管理员/会话/CSRF、演示模式、写入式凭据与加密存储、签名、授权链接、签到、搜索排序与完整字段（含数组、非 115 不可选）、空结果不回落演示数据、积分上限拦截、115 分页接收与本地 fid 回填、机器令牌（只显示一次、哈希存储、轮换失效、权限范围）、订阅只列候选不解锁、停用/删除、退出。

协议测试对照原客户端回放中的已知 HMAC 向量；本地 HTTP 集成测试覆盖真实序列化请求、积分拦截、115 多页文件/目录接收、去重、异常结果保留、重启恢复、会话与配置加密。

本次对默认中转服务仅执行了未授权安装标识的只读状态请求，收到 HTTP 200 / `REAUTH_REQUIRED`。未使用真实 RE0 与 115 账号完成解锁、扣分或转存联调，因此账户相关字段和第三方服务行为仍需连接账号后验证。Docker 文件已提供，当前开发环境无 Docker，尚未构建验证镜像。
