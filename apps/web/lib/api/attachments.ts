/**
 * Attachments API Service (JWT-based auth)
 */

import { API_PREFIX } from "./config";

// Function to get JWT token - will be set by the store
let getToken: (() => string | null) | null = null;

export const setAttachmentSessionIdGetter = (getter: () => string | null) => {
  getToken = getter;
};

// Allowed upload types. Must be a subset of the server list (crates/api/src/routes/attachments.rs
// `upload_type_allowed`); e2e/tests/attachment-types.spec.ts checks it.
export const ALLOWED_IMAGE_TYPES = ['image/jpeg', 'image/png', 'image/gif', 'image/webp', 'image/avif', 'image/bmp'];
export const ALLOWED_DOCUMENT_TYPES = [
  'application/pdf', 'text/plain', 'text/csv', 'application/zip', 'application/x-zip-compressed',
  'application/msword', 'application/vnd.ms-excel', 'application/vnd.ms-powerpoint',
];
export const ALLOWED_FILE_TYPES = [...ALLOWED_IMAGE_TYPES, ...ALLOWED_DOCUMENT_TYPES];
// Server accepts these prefixes too (openxml / opendocument); File.type is matched by prefix.
export const ALLOWED_TYPE_PREFIXES = [
  'application/vnd.openxmlformats-officedocument.',
  'application/vnd.oasis.opendocument.',
];

export const isAllowedFileType = (file: File): boolean => {
  return ALLOWED_FILE_TYPES.includes(file.type) || ALLOWED_TYPE_PREFIXES.some((p) => file.type.startsWith(p));
};

export const attachmentsApi = {
  /**
   * Get attachment file URL
   */
  getFileUrl: (attachmentId: string): string => {
    return `${API_PREFIX}/attachments/file/${attachmentId}`;
  },

  /**
   * Fetch attachment as blob with authorization
   */
  getFile: async (attachmentId: string): Promise<Blob> => {
    const url = `${API_PREFIX}/attachments/file/${attachmentId}`;
    const token = getToken?.();
    
    const headers: Record<string, string> = {};
    if (token) {
      headers["Authorization"] = `Bearer ${token}`;
    }
    
    const response = await fetch(url, {
      method: 'GET',
      headers,
      credentials: 'include',
    });
    
    if (!response.ok) {
      throw new Error('Failed to fetch attachment');
    }
    return response.blob();
  },

  /**
   * Upload attachment to a ticket
   * @param ticketId - The ticket ID to attach the file to
   * @param file - The file to upload
   * @param options - Optional message and sendEmail flag
   */
  upload: async (
    ticketId: string,
    file: File,
    options?: { message?: string; sendEmail?: boolean }
  ): Promise<{ attachmentId: string; messageId?: string }> => {
    const url = `${API_PREFIX}/attachments/${ticketId}`;
    const token = getToken?.();
    
    const headers: Record<string, string> = {
      "Content-Type": file.type || "application/octet-stream",
      "X-Filename": file.name,
    };
    
    if (token) {
      headers["Authorization"] = `Bearer ${token}`;
    }
    
    if (options?.message) {
      headers["X-Message"] = options.message;
    }
    
    if (options?.sendEmail) {
      headers["X-Send-Email"] = "true";
    }
    
    const response = await fetch(url, {
      method: 'PUT',
      headers,
      body: file,
      credentials: 'include',
    });
    
    if (!response.ok) {
      const errorData = await response.json().catch(() => ({ error: 'Failed to upload attachment' }));
      // Handle various error formats: { error: { message: "..." } }, { error: "..." }, { message: "..." }
      const errorMessage = 
        errorData.error?.message || 
        (typeof errorData.error === 'string' ? errorData.error : null) ||
        errorData.message || 
        'Failed to upload attachment';
      throw new Error(errorMessage);
    }
    
    const body = await response.json();
    return body.data ?? body;
  },

  /**
   * Upload multiple attachments to a ticket in one message
   * @param ticketId - The ticket ID to attach the files to
   * @param files - Array of files to upload
   * @param options - Optional message and sendEmail flag
   */
  uploadMultiple: async (
    ticketId: string,
    files: File[],
    options?: { message?: string; sendEmail?: boolean }
  ): Promise<{ attachmentIds: string[]; messageId?: string }> => {
    const url = `${API_PREFIX}/attachments/${ticketId}/multiple`;
    const token = getToken?.();
    
    const formData = new FormData();
    // Use 'files' as field name (not 'files[]')
    files.forEach((file) => {
      formData.append('files', file);
    });
    
    // Add message and sendEmail to form data body
    if (options?.message) {
      formData.append('message', options.message);
    }
    
    if (options?.sendEmail) {
      formData.append('sendEmail', 'true');
    }
    
    const headers: Record<string, string> = {};
    
    if (token) {
      headers["Authorization"] = `Bearer ${token}`;
    }
    
    const response = await fetch(url, {
      method: 'POST',
      headers,
      body: formData,
      credentials: 'include',
    });
    
    if (!response.ok) {
      const errorData = await response.json().catch(() => ({ error: 'Failed to upload attachments' }));
      // Handle various error formats: { error: { message: "..." } }, { error: "..." }, { message: "..." }
      const errorMessage = 
        errorData.error?.message || 
        (typeof errorData.error === 'string' ? errorData.error : null) ||
        errorData.message || 
        'Failed to upload attachments';
      throw new Error(errorMessage);
    }
    
    const body = await response.json();
    return body.data ?? body;
  },
};
