import { describe, expect, it } from 'vitest'
import { DEFAULT_LOCALE, messages } from './i18n'

describe('view translations', () => {
  it('defaults to English and exposes matching Chinese UI copy', () => {
    expect(DEFAULT_LOCALE).toBe('en')
    expect(messages.en.overview).toBe('Overview')
    expect(messages['zh-CN'].overview).toBe('概览')
    expect(messages['zh-CN'].readOnly).toBe('只读')
  })
})
