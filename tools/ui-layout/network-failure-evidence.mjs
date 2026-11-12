const MOCK_DATA_VISUAL_FIXTURE_URLS = Object.freeze([
  'https://openai.com/favicon.ico',
  'https://www.rust-lang.org/static/images/favicon.ico',
  'https://arstechnica.com/favicon.ico',
  'https://www.nasa.gov/favicon.ico',
  'https://unsplash.com/favicon.ico',
  'https://github.githubassets.com/favicons/favicon.png',
  'https://picsum.photos/seed/deepseek/160/120',
  'https://picsum.photos/seed/windows11/160/120',
  'https://picsum.photos/seed/nebula/600/800',
  'https://picsum.photos/seed/arch/600/450',
  'https://picsum.photos/seed/tokyo/600/750',
  'https://picsum.photos/seed/podcast1/120/120',
]);

const CONTROLLED_MEDIA_FIXTURE_PATHS = Object.freeze([
  '/gallery-image.svg',
  '/delayed-image.svg',
  '/failed-image.svg',
  '/poster.svg',
  '/video.mp4',
]);

function uniqueUrls(values) {
  return [...new Set(values.filter((value) => typeof value === 'string' && value.length > 0))];
}

function findFixtureRule(failure, policy) {
  const urls = uniqueUrls([failure.initialUrl, ...failure.requestUrls, failure.url]);
  const exactUrl = urls.find((url) => policy.expectedUrls?.includes(url));
  if (exactUrl) return { kind: 'mock-data-visual', url: exactUrl };

  if (!policy.mediaOrigin) return null;
  const mediaOrigin = new URL(policy.mediaOrigin).origin;
  for (const rawUrl of urls) {
    try {
      const url = new URL(rawUrl);
      if (url.origin === mediaOrigin && policy.mediaPaths?.includes(url.pathname)) {
        return { kind: 'controlled-media', url: rawUrl };
      }
    } catch {
      continue;
    }
  }
  return null;
}

export function createNetworkFailureTracker(cdp) {
  const requests = new Map();
  const failures = [];
  const offRequest = cdp.on('Network.requestWillBeSent', (event) => {
    const current = requests.get(event.requestId) ?? {
      initialUrl: null,
      requestUrls: [],
      resourceType: null,
    };
    const url = event.request?.url ?? null;
    if (url && current.initialUrl === null) current.initialUrl = url;
    if (url && current.requestUrls.at(-1) !== url) current.requestUrls.push(url);
    current.resourceType = event.type ?? current.resourceType;
    requests.set(event.requestId, current);
  });
  const offFailure = cdp.on('Network.loadingFailed', (event) => {
    const request = requests.get(event.requestId);
    failures.push({
      event: 'Network.loadingFailed',
      at: new Date().toISOString(),
      type: event.type ?? null,
      requestId: event.requestId ?? null,
      initialUrl: request?.initialUrl ?? null,
      url: request?.requestUrls.at(-1) ?? request?.initialUrl ?? null,
      requestUrls: request?.requestUrls.slice() ?? [],
      resourceType: request?.resourceType ?? event.type ?? null,
      failure: event.errorText ?? 'unknown',
      errorText: event.errorText ?? null,
      blockedReason: event.blockedReason ?? null,
      canceled: Boolean(event.canceled),
    });
  });

  return {
    reset() {
      requests.clear();
      failures.length = 0;
    },
    finalize(policy = {}) {
      const classified = failures.map((failure) => {
        const attributable = Boolean(
          failure.requestId
          && failure.url
          && failure.resourceType
          && failure.failure,
        );
        const fixture = attributable
          ? findFixtureRule(failure, policy)
          : null;
        return {
          ...failure,
          expectedFixture: Boolean(fixture),
          fixtureRule: fixture,
        };
      });
      return {
        networkFailurePolicy: {
          expectedUrls: policy.expectedUrls ?? [],
          mediaOrigin: policy.mediaOrigin ?? null,
          mediaPaths: policy.mediaOrigin ? (policy.mediaPaths ?? []) : [],
        },
        networkFailures: classified,
        expectedNetworkFailures: classified.filter((failure) => failure.expectedFixture),
        unexpectedNetworkFailures: classified.filter((failure) => !failure.expectedFixture),
      };
    },
    close() {
      offRequest();
      offFailure();
    },
  };
}

export {
  CONTROLLED_MEDIA_FIXTURE_PATHS,
  MOCK_DATA_VISUAL_FIXTURE_URLS,
};
