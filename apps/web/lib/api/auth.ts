/**
 * Auth API Service (JWT-based with refresh token rotation)
 */

import { API_PREFIX } from './config';

export interface User {
  id: string;
  email: string;
  name: string;
  role: string;
  status?: string;
  phone?: string;
  timezone?: string;
  avatarUrl?: string | null;
  lastLoginAt?: string;
  createdAt?: string;
  updatedAt?: string;
}

export interface SignUpRequest {
  email: string;
  password: string;
  name: string;
}

export interface SignInRequest {
  email: string;
  password: string;
}

export interface SignInResponse {
  user: User;
  accessToken: string;
  expiresIn: number;
  refreshToken?: string; // Returned for cross-origin/cross-subdomain setups (stored in localStorage)
}

export interface RefreshResponse {
  user: User;
  accessToken: string;
  expiresIn: number;
}

// Real envelope: { success: true, data } | { success: false, error: { code, message } }
export interface AuthApiResponse<T> {
  success: boolean;
  data?: T;
  error?: { code?: string; message?: string };
}

const failure = (message: string, code: string): AuthApiResponse<never> => ({
  success: false,
  error: { code, message },
});

// Parse the envelope; a non-JSON body (proxy error page, 502) becomes a failure instead of throwing.
const parse = async <T>(response: Response): Promise<AuthApiResponse<T>> => {
  try {
    const body = await response.json();
    if (typeof body?.success === 'boolean') return body;
  } catch {
    // fall through
  }
  return failure(`Request failed (${response.status})`, 'BAD_RESPONSE');
};

// Auth API uses credentials: 'include' for refresh token cookies
export const authApi = {
  /**
   * Create a new user account
   */
  signUp: async (data: SignUpRequest): Promise<AuthApiResponse<User>> => {
    const response = await fetch(`${API_PREFIX}/auth/signup`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'include',
      body: JSON.stringify(data),
    });
    return parse<User>(response);
  },

  /**
   * Sign in and receive access token (refresh token set via HTTP-only cookie)
   */
  signIn: async (data: SignInRequest): Promise<AuthApiResponse<SignInResponse>> => {
    try {
      const response = await fetch(`${API_PREFIX}/auth/signin`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: JSON.stringify(data),
      });
      return await parse<SignInResponse>(response);
    } catch (err) {
      console.error("[AuthAPI] SignIn request failed:", err)
      // Return error response format
      return failure(err instanceof Error ? err.message : 'Network error', 'NETWORK_ERROR');
    }
  },

  /**
   * Sign out and invalidate refresh token
   */
  signOut: async (): Promise<AuthApiResponse<{ message: string }>> => {
    const response = await fetch(`${API_PREFIX}/auth/signout`, {
      method: 'POST',
      credentials: 'include',
    });
    return parse<{ message: string }>(response);
  },

  /**
   * Refresh the access token using the HttpOnly refresh cookie (same-origin deployment).
   */
  refresh: async (): Promise<AuthApiResponse<RefreshResponse>> => {
    const response = await fetch(`${API_PREFIX}/auth/refresh`, {
      method: "POST",
      credentials: "include",
    });
    return parse<RefreshResponse>(response);
  },

  /**
   * Get current authenticated user
   */
  me: async (accessToken: string): Promise<AuthApiResponse<{ user: User }>> => {
    const response = await fetch(`${API_PREFIX}/auth/me`, {
      method: 'GET',
      headers: {
        'Authorization': `Bearer ${accessToken}`,
      },
      credentials: 'include',
    });
    return parse<{ user: User }>(response);
  },
};
