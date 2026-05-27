import { createContext, useContext, useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { tryGetCurrentWindow } from '../tauri/runtime';
import type { SettingsSnapshot, SettingsSnapshotEvent } from '../settings/types';
import en from './locales/en.json';
import zh from './locales/zh.json';

type Language = 'en' | 'zh';

interface I18nContextType {
  language: Language;
  configLanguage: 'auto' | 'en' | 'zh';
  t: (key: string, replacements?: Record<string, string>) => string;
}

const I18nContext = createContext<I18nContextType | null>(null);

const locales: Record<Language, Record<string, string>> = {
  en: en as Record<string, string>,
  zh: zh as Record<string, string>,
};

const detectSystemLanguage = (): Language => {
  const lang = navigator.language || 'en';
  return lang.toLowerCase().startsWith('zh') ? 'zh' : 'en';
};

export function I18nProvider({ children }: { children: ReactNode }) {
  const [configLanguage, setConfigLanguage] = useState<'auto' | 'en' | 'zh'>('auto');

  useEffect(() => {
    let mounted = true;

    // Load initial settings snapshot to read configuration language
    invoke<SettingsSnapshot>('get_settings_snapshot')
      .then((snapshot) => {
        if (!mounted) return;
        const lang = (snapshot?.values as Record<string, unknown> | undefined)?.language;
        if (lang === 'zh' || lang === 'en' || lang === 'auto') {
          setConfigLanguage(lang);
        }
      })
      .catch(() => {});

    // Listen to settings changes to dynamically update language in real-time
    const currentWindow = tryGetCurrentWindow();
    let unlisten: (() => void) | null = null;

    if (currentWindow) {
      void currentWindow
        .listen<SettingsSnapshotEvent>('config_snapshot_changed', (event) => {
          if (!mounted) return;
          const lang = (event.payload?.values as Record<string, unknown> | undefined)?.language;
          if (lang === 'zh' || lang === 'en' || lang === 'auto') {
            setConfigLanguage(lang);
          }
        })
        .then((dispose) => {
          unlisten = dispose;
        });
    }

    return () => {
      mounted = false;
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  const resolvedLanguage: Language = configLanguage === 'auto' ? detectSystemLanguage() : configLanguage;

  const t = (key: string, replacements?: Record<string, string>): string => {
    const langData = locales[resolvedLanguage];
    let value = langData[key] || locales['en'][key] || key;

    if (replacements) {
      Object.entries(replacements).forEach(([k, v]) => {
        value = value.replace(new RegExp(`{${k}}`, 'g'), v);
      });
    }

    return value;
  };

  return (
    <I18nContext.Provider value={{ language: resolvedLanguage, configLanguage, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n() {
  const context = useContext(I18nContext);
  if (!context) {
    throw new Error('useI18n must be used within an I18nProvider');
  }
  return context;
}
