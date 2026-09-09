import type { CatalogShape } from '../catalog'

export const providerConfigZhCN = {
  /** Seed rows, read when the store writes its first state file. */
  builtIn: {
    anthropic: 'Anthropic 官方',
    openai: 'OpenAI 官方'
  },

  notFound: '供应商配置不存在',
  sessionProviderNotFound: '会话绑定的供应商配置不存在',
  activeCannotBeDeleted: '使用中的供应商需要先切换后再删除',
  builtInKept: '官方供应商配置保留为默认入口',
  invalidCli: 'CLI 类型不正确',
  nameRequired: '供应商名称不能为空',
  modelRequired: '默认模型不能为空',
  endpointInvalid: 'API 地址需要使用有效的 HTTP 或 HTTPS 地址'
}

export const providerConfigEn: CatalogShape<typeof providerConfigZhCN> = {
  builtIn: {
    anthropic: 'Anthropic (official)',
    openai: 'OpenAI (official)'
  },

  notFound: 'That provider configuration does not exist',
  sessionProviderNotFound: 'The provider configuration bound to this session does not exist',
  activeCannotBeDeleted: 'Switch away from the provider in use before deleting it',
  builtInKept: 'The official provider configuration stays as the default entry',
  invalidCli: 'The CLI type is invalid',
  nameRequired: 'The provider name must not be empty',
  modelRequired: 'The default model must not be empty',
  endpointInvalid: 'The API endpoint must be a valid HTTP or HTTPS address'
}
