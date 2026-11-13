export const providerOptions = [
  {
    id: 'groq-whisper-turbo',
    label: 'Groq',
    model: 'whisper-large-v3-turbo'
  },
  {
    id: 'openai-gpt-4o-mini-transcribe',
    label: 'OpenAI',
    model: 'gpt-transcribe'
  },
  {
    id: 'xai-stt',
    label: 'xAI',
    model: 'grok-voice-transcribe-2.0'
  },
  {
    id: 'elevenlabs-scribe-v2',
    label: 'ElevenLabs',
    model: 'scribe_v2'
  },
  {
    id: 'deepgram-nova-3',
    label: 'Deepgram',
    model: 'nova-3'
  },
  {
    id: 'mistral-voxtral-3',
    label: 'Mistral',
    model: 'voxtral-mini-transcribe-3-0'
  }
] as const;

export type ProviderId = (typeof providerOptions)[number]['id'];
export type ProviderKeyStatus = 'unchecked' | 'valid' | 'invalid';
type ProviderKeyStatuses = Partial<Record<ProviderId, ProviderKeyStatus>>;

export type EchoSettings = {
  pushToTalkShortcut: string;
  fallbackShortcut?: string;
  providerId: ProviderId;
  providerKeyStatuses?: ProviderKeyStatuses;
  providerKeySaved?: Partial<Record<ProviderId, boolean>>;
  phrasebookTerms?: string[];
  language?: string;
  customPrompt?: string;
  pasteAfterTranscription: boolean;
  restoreClipboard: boolean;
  debugLoggingEnabled: boolean;
  runInBackground: boolean;
  launchAtLogin: boolean;
  historyRetentionDays?: number;
};

const MACOS_DEFAULT_PUSH_TO_TALK_SHORTCUT = 'Fn';
const WINDOWS_DEFAULT_PUSH_TO_TALK_SHORTCUT = 'Control+Windows';

function defaultPushToTalkShortcut() {
  const platform = typeof navigator !== 'undefined' ? navigator.platform.toLowerCase() : '';
  const userAgent = typeof navigator !== 'undefined' ? navigator.userAgent.toLowerCase() : '';
  return platform.includes('win') || userAgent.includes('windows')
    ? WINDOWS_DEFAULT_PUSH_TO_TALK_SHORTCUT
    : MACOS_DEFAULT_PUSH_TO_TALK_SHORTCUT;
}

export const defaultSettings: EchoSettings = {
  pushToTalkShortcut: defaultPushToTalkShortcut(),
  fallbackShortcut: 'CommandOrControl+Shift+Space',
  providerId: 'groq-whisper-turbo',
  providerKeySaved: Object.fromEntries(providerOptions.map((provider) => [provider.id, false])),
  pasteAfterTranscription: true,
  restoreClipboard: true,
  debugLoggingEnabled: false,
  runInBackground: false,
  launchAtLogin: false,
  historyRetentionDays: 30
};
