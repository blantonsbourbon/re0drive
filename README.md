# RE0Drive

个人自托管的 RE0 订阅、资源搜索和 115 分享转存 App。使用 TgtoDrive 的中转协议，默认服务为 `https://re0.tgtodrive.top`，不依赖申请 RE0 官方开发者 API。

后端采用 Rust + Axum + Tokio，SQLite 保存订阅和队列；网页无需 Node 构建，随单个可执行文件发布。HTTP 连接复用，中转查询最多 4 个并发，写入外部服务的任务串行处理。语言和框架没有通用的“性能最好”，这里优先降低常驻开销，外部服务的响应速度与限流仍是主要约束。

## 功能

- 片名搜索（配置 TMDB Key），或直接使用 `tmdb:693134` / 数字 ID 搜索。
- 电影、剧集订阅；版本、包含/排除关键词、积分上限、目标目录和检查间隔。
- 每条订阅每次最多新增 10 个任务，默认每 30 分钟检查。
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

1. 在“连接与设置”保留默认中转地址，保存配置。
2. 点击“授权 RE0”，打开生成的链接，在 RE0 完成 OAuth 授权，回到 App 检查授权状态。
3. 填写目标 115 账号的 Cookie，例如 `UID=…; CID=…; SEID=…; KID=…`。这里只使用 Cookie 分享接收接口，不申请 115 OpenAPI。
4. 目标目录 ID 为 `0` 时接收到根目录。已有目录 ID 可从 115 的目录页面 URL 获取。
5. 配置 TMDB API Key可按片名搜索；不配置时仍可用数字 TMDB ID 查询。
6. 搜索资源手动转存，或创建自动转存订阅。

Cookie 失效时需要在设置中更新。更换中转地址、安装标识、签名密钥、115 Cookie 或连接模式后，旧队列不会继续使用新的账号上下文；请重新创建任务。

## Docker / NAS

```sh
cp .env.example .env
docker compose up -d --build
```

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
| `RE0DRIVE_TMDB_BASE_URL` | `https://api.themoviedb.org` | TMDB 接口地址；仅用于离线测试或自建代理 |
| `TZ` | 镜像为 `Asia/Hong_Kong` | 每日签到使用的本地时区 |
| `RUST_LOG` | `re0drive=info,tower_http=info` | 日志级别 |

只读诊断：`./re0drive --check-gateway`（源码构建路径为 `./target/release/re0drive --check-gateway`）。检查授权状态，不解锁资源、不签到、不转存。

## 积分和任务状态

本地积分上限是**请求解锁之前的预检查**，中转解锁 body 只接收 `slug`，没有服务端原子性的最大积分参数。查询与解锁之间若价格变化，服务器实际计费可能变化。费用未知、负数或超过上限时跳过；已拥有资源仍请求服务器返回链接。

`queued → checking → unlocking → transferring → success`。检查阶段出错为 `failed`，可重试。解锁或接收响应丢失、不完整、服务重启中断写入时为 `needs_review`，需要在 RE0 和 115 核对后再手工重试。停用或删除订阅时，尚未执行的任务会跳过；已开始的外部请求会完成。

当前实现支持 **115 分享链接转存**。ED2K 离线任务、字幕筛选、整季/单集语义、115 目录整理和多账号自动切换不在本版范围内；版本与关键词筛选作用于资源标题/画质字段。

## 验证

```sh
cargo test --locked                       # 16 项：协议向量 + 本地 HTTP 流程
cargo build --release --locked && tests/smoke.sh   # 34 项端到端冒烟测试
```

`tests/smoke.sh` 自带环境：在随机本地端口启动 `tests/mock_upstream.py`（RE0 中转 + 115 + TMDB 替身，用测试密钥校验每个请求的 HMAC）和一个全新数据目录的 RE0Drive，所有外部地址都指向替身；同时把代理设为一个关闭的本地端口作为“绊线”，任何意外的非本机请求都会直接失败，不会离开本机。覆盖：静态资源与 CSP、管理员/会话/CSRF、演示模式、写入式凭据与加密存储、签名、授权链接、签到、搜索排序、积分上限拦截、115 分页接收、订阅自动转存、停用/删除、退出。

协议测试对照原客户端回放中的已知 HMAC 向量；本地 HTTP 集成测试覆盖真实序列化请求、积分拦截、115 多页文件/目录接收、去重、异常结果保留、重启恢复、会话与配置加密。

本次对默认中转服务仅执行了未授权安装标识的只读状态请求，收到 HTTP 200 / `REAUTH_REQUIRED`。未使用真实 RE0 与 115 账号完成解锁、扣分或转存联调，因此账户相关字段和第三方服务行为仍需连接账号后验证。Docker 文件已提供，当前开发环境无 Docker，尚未构建验证镜像。
