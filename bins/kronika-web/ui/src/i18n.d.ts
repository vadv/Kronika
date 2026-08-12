declare module "kronika:i18n" {
  export const dictionaries: {
    readonly en: Readonly<Record<string, string>>
    readonly ru: Readonly<Record<string, string>>
  }
  export type TranslationKey = keyof typeof dictionaries.en
}
