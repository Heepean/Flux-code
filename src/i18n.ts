import en from './locales/en.json'
import ru from './locales/ru.json'

export type Language = 'en' | 'ru'
export type TranslationKey = keyof typeof en

const dictionaries = { en, ru }

export function translate(language: Language, key: TranslationKey, values?: Record<string, string | number>) {
  const template = dictionaries[language][key] ?? en[key]
  return values ? template.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(values[name] ?? '')) : template
}
