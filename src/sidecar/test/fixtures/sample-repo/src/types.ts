/**
 * User types for testing the type-sidecar bundler
 */

export interface User {
  id: string;
  name: string;
  email: string;
  createdAt: Date;
}

export interface UserProfile extends User {
  bio?: string;
  avatar?: string;
  settings: UserSettings;
}

export interface UserSettings {
  theme: 'light' | 'dark';
  notifications: boolean;
  language: string;
}

export type UserId = string;

export type UserRole = 'admin' | 'user' | 'guest';
