import type {
  AccountUsageReadResult,
  DashboardSnapshot,
  LocalUsageSummary,
  RateLimitsReadResult,
} from "./types.js";
import {
  AppServerClient,
  AppServerProcessExitError,
  AppServerRequestTimeoutError,
  AppServerRpcError,
  CodexExecutableNotFoundError,
  normalizeRateLimits,
  type AccountReadResult,
} from "./app-server/index.js";

export interface DashboardAppServerClient {
  readonly serverUserAgent: string;
  accountRead(refreshToken?: boolean): Promise<AccountReadResult>;
  rateLimitsRead(): Promise<RateLimitsReadResult>;
  usageRead(): Promise<AccountUsageReadResult>;
  close(): void;
}

export interface DashboardReadOptions {
  connect?: () => Promise<DashboardAppServerClient>;
  readLocalUsage?: () => Promise<LocalUsageSummary | null>;
  now?: () => Date;
}

function emptySnapshot(
  status: DashboardSnapshot["status"],
  fetchedAt: string,
  localUsage: LocalUsageSummary | null,
  message: string,
): DashboardSnapshot {
  return {
    status,
    fetchedAt,
    codexVersion: null,
    quotaWindows: [],
    accountDiagnostics: null,
    planRenewalAt: null,
    planRenewalSource: null,
    resetCredits: null,
    accountUsage: null,
    localUsage,
    message,
  };
}

function codexVersionFromUserAgent(userAgent: string): string | null {
  return userAgent.match(/\d+\.\d+\.\d+(?:-[\w.-]+)?/)?.[0] ?? null;
}

function isUnsupportedMethod(error: unknown): boolean {
  return error instanceof AppServerRpcError &&
    (error.code === -32601 || /method not found|unsupported/i.test(error.message));
}

function connectionFailureStatus(error: unknown): {
  status: DashboardSnapshot["status"];
  message: string;
} {
  if (error instanceof CodexExecutableNotFoundError) {
    return {
      status: "unsupported",
      message: "未找到可用的 Codex 命令行程序；本机历史统计仍可使用。",
    };
  }
  if (
    error instanceof AppServerProcessExitError ||
    error instanceof AppServerRequestTimeoutError
  ) {
    return {
      status: "offline",
      message: "暂时无法连接 Codex App Server；本机历史统计仍可使用。",
    };
  }
  return {
    status: "error",
    message: "读取 Codex 服务端用量失败；未记录原始错误内容。",
  };
}

async function safelyReadLocalUsage(
  reader: DashboardReadOptions["readLocalUsage"],
): Promise<{ usage: LocalUsageSummary | null; failed: boolean }> {
  if (!reader) return { usage: null, failed: false };
  try {
    return { usage: await reader(), failed: false };
  } catch {
    return { usage: null, failed: true };
  }
}

/**
 * Builds the single sanitized snapshot consumed by the UI. Raw account data,
 * App Server responses, stderr and rollout contents never leave this layer.
 */
export async function readDashboardSnapshot(
  options: DashboardReadOptions = {},
): Promise<DashboardSnapshot> {
  const now = options.now ?? (() => new Date());
  const fetchedAt = now().toISOString();
  const accountReadStartedAt = Date.now();
  const methods: string[] = [];
  const withAccountDiagnostics = (snapshot: DashboardSnapshot): DashboardSnapshot => ({
    ...snapshot,
    accountDiagnostics: {
      readAt: fetchedAt,
      durationMs: Math.max(0, Date.now() - accountReadStartedAt),
      methods: [...methods],
    },
  });
  const localPromise = safelyReadLocalUsage(options.readLocalUsage);
  const connect = options.connect ?? (() => AppServerClient.connect());
  let client: DashboardAppServerClient | null = null;

  try {
    client = await connect();
  } catch (error) {
    const local = await localPromise;
    const failure = connectionFailureStatus(error);
    return withAccountDiagnostics(
      emptySnapshot(failure.status, fetchedAt, local.usage, failure.message),
    );
  }

  try {
    const local = await localPromise;
    let account: AccountReadResult;
    try {
      methods.push("account/read");
      account = await client.accountRead(false);
    } catch (error) {
      const failure = connectionFailureStatus(error);
      return withAccountDiagnostics({
        ...emptySnapshot(failure.status, fetchedAt, local.usage, failure.message),
        codexVersion: codexVersionFromUserAgent(client.serverUserAgent),
      });
    }

    if (account.requiresOpenaiAuth && account.account === null) {
      return withAccountDiagnostics({
        ...emptySnapshot(
          "unauthenticated",
          fetchedAt,
          local.usage,
          "Codex 尚未登录；登录后可读取账号额度，本机历史统计仍可使用。",
        ),
        codexVersion: codexVersionFromUserAgent(client.serverUserAgent),
      });
    }

    methods.push("account/rateLimits/read", "account/usage/read");
    const [rateLimitsResult, accountUsageResult] = await Promise.allSettled([
      client.rateLimitsRead(),
      client.usageRead(),
    ]);
    const rateLimits = rateLimitsResult.status === "fulfilled"
      ? rateLimitsResult.value
      : null;
    const accountUsage = accountUsageResult.status === "fulfilled"
      ? accountUsageResult.value
      : null;
    const failures = [rateLimitsResult, accountUsageResult].filter(
      (result): result is PromiseRejectedResult => result.status === "rejected",
    );

    if (failures.length === 2) {
      const unsupported = failures.every((failure) => isUnsupportedMethod(failure.reason));
      return withAccountDiagnostics({
        ...emptySnapshot(
          unsupported ? "unsupported" : "error",
          fetchedAt,
          local.usage,
          unsupported
            ? "当前 Codex 版本不支持所需的账号用量接口；本机历史统计仍可使用。"
            : "账号用量接口读取失败；未记录原始错误内容。",
        ),
        codexVersion: codexVersionFromUserAgent(client.serverUserAgent),
      });
    }

    const messages: string[] = [];
    if (failures.length > 0) messages.push("部分账号用量暂不可用。");
    if (local.failed) messages.push("本机日志索引暂不可用。");

    return withAccountDiagnostics({
      status: "ready",
      fetchedAt,
      codexVersion: codexVersionFromUserAgent(client.serverUserAgent),
      quotaWindows: rateLimits ? normalizeRateLimits(rateLimits) : [],
      // App Server does not expose subscription renewal dates. In particular,
      // quota-window reset timestamps must not be presented as plan renewals.
      planRenewalAt: null,
      planRenewalSource: null,
      resetCredits: rateLimits?.rateLimitResetCredits ?? null,
      accountUsage,
      localUsage: local.usage,
      message: messages.length > 0 ? messages.join(" ") : null,
    });
  } finally {
    client.close();
  }
}
