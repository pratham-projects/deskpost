/**
 * API Client - Centralized HTTP client for all API calls (JWT-based auth with refresh)
 */

import { API_BASE_URL, API_VERSION } from "./config";

export interface ApiResponse<T> {
  success: boolean;
  data: T;
}

/** Thrown when the session is gone (refresh failed). The logout handler has already run. */
export class AuthExpiredError extends Error {
  constructor() {
    super("Session expired");
    this.name = "AuthExpiredError";
  }
}

const AUTH_CODES = new Set(["TOKEN_EXPIRED", "TOKEN_INVALID", "TOKEN_MISSING"]);

// Function to get JWT token from auth store (will be set after store is initialized)
let getToken: (() => string | null) | null = null;
// Function to check if token is expired
let isTokenExpired: (() => boolean) | null = null;
// Function to refresh the token
let refreshToken: (() => Promise<boolean>) | null = null;
// Function to handle logout (will be set after store is initialized)
let handleLogout: (() => void) | null = null;

export const setTokenGetter = (getter: () => string | null) => {
  getToken = getter;
};

export const setTokenExpiredChecker = (checker: () => boolean) => {
  isTokenExpired = checker;
};

export const setTokenRefresher = (refresher: () => Promise<boolean>) => {
  refreshToken = refresher;
};

export const setLogoutHandler = (handler: () => void) => {
  handleLogout = handler;
};

/** Non-auth API failure; `details` carries 422 field messages from the server. */
export class ApiRequestError extends Error {
  constructor(
    message: string,
    public status: number,
    public code: string,
    public details?: unknown
  ) {
    super(message);
    this.name = "ApiRequestError";
  }
}

class ApiClient {
  private baseUrl: string;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  private async request<T>(
    endpoint: string,
    options: RequestInit = {},
    isRetry = false
  ): Promise<T> {
    const url = `${this.baseUrl}${endpoint}`;

    // Proactively refresh token if expired (skip for auth endpoints)
    const isAuthEndpoint = /^\/api\/[^/]+\/auth\//.test(endpoint);
    if (!isAuthEndpoint && isTokenExpired?.() && refreshToken) {
      if (!(await refreshToken())) {
        handleLogout?.();
        throw new AuthExpiredError();
      }
    }

    // Get JWT token if available
    const token = getToken?.();

    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...(options.headers as Record<string, string>),
    };

    // Add Authorization Bearer token if available
    if (token) {
      headers["Authorization"] = "Bearer " + token;
    }

    const config: RequestInit = {
      ...options,
      headers,
      credentials: 'include', // Include cookies for refresh token
    };


    let response: Response;
    try {
      response = await fetch(url, config);
    } catch {
      // Handle network errors (offline, DNS issues, etc.)
      // Don't logout on network errors - user might just be temporarily offline
      // Let the calling code handle the error gracefully
      throw new Error("Network connection failed");
    }

    if (!response.ok) {
      // Envelope: { success: false, error: { code, message, details? } }
      const errorData = await response.json().catch(() => ({}));
      const errorCode: string = errorData?.error?.code ?? "";
      const errorMessage: string =
        errorData?.error?.message || errorData?.message || "An unexpected error occurred";

      if (response.status === 401 && AUTH_CODES.has(errorCode) && !isAuthEndpoint) {
        if (!isRetry && refreshToken && (await refreshToken())) {
          return this.request<T>(endpoint, options, true);
        }
        handleLogout?.();
        throw new AuthExpiredError();
      }

      throw new ApiRequestError(errorMessage, response.status, errorCode, errorData?.error?.details);
    }

    return response.json();
  }

  async get<T>(endpoint: string, params?: Record<string, string>): Promise<T> {
    const queryString = params
      ? '?' + new URLSearchParams(params).toString()
      : '';
    return this.request<T>(`${endpoint}${queryString}`, { method: 'GET' });
  }

  async post<T>(endpoint: string, data?: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: 'POST',
      body: data ? JSON.stringify(data) : undefined,
    });
  }

  async patch<T>(endpoint: string, data: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: 'PATCH',
      body: JSON.stringify(data),
    });
  }

  async delete<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint, { method: 'DELETE' });
  }
}

export const apiClient = new ApiClient(API_BASE_URL);
export { API_BASE_URL, API_VERSION };

// Legacy alias for backward compatibility
export const setSessionIdGetter = setTokenGetter;
