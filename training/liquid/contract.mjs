// liquid-agent/v1 constants and the vendored schemas (training/liquid/contract), shared with
// Infernet Protocol and c0mpute. Ported from infernet-protocol packages/liquid-agent/src/contract.js
// at the commit named in contract/README.md. The schemas are imported as JSON modules so a
// compiled `w3bs-training` binary carries them with no files beside it.
import actionProposal from './contract/schemas/action-proposal.schema.json' with { type: 'json' };
import common from './contract/schemas/common.schema.json' with { type: 'json' };
import datasetBundle from './contract/schemas/dataset-bundle.schema.json' with { type: 'json' };
import datasetManifest from './contract/schemas/dataset-manifest.schema.json' with { type: 'json' };
import episode from './contract/schemas/episode.schema.json' with { type: 'json' };
import inferenceSession from './contract/schemas/inference-session.schema.json' with { type: 'json' };
import modelManifest from './contract/schemas/model-manifest.schema.json' with { type: 'json' };
import predictRequest from './contract/schemas/predict-request.schema.json' with { type: 'json' };
import trainJob from './contract/schemas/train-job.schema.json' with { type: 'json' };

export const SCHEMA_VERSION = 'liquid-agent/v1';
export const CONTRACT_SOURCE = {
  repository: 'https://github.com/InfernetProtocol/infernet-protocol',
  commit: '81060e1ee2c5bce7c3f87499ecf3b01aa5e35708',
};

// file name -> parsed schema, as Infernet hashes them for contract.json schemas_hash.
export const SCHEMAS = Object.freeze({
  'action-proposal.schema.json': actionProposal,
  'common.schema.json': common,
  'dataset-bundle.schema.json': datasetBundle,
  'dataset-manifest.schema.json': datasetManifest,
  'episode.schema.json': episode,
  'inference-session.schema.json': inferenceSession,
  'model-manifest.schema.json': modelManifest,
  'predict-request.schema.json': predictRequest,
  'train-job.schema.json': trainJob,
});

export const WORKLOADS = Object.freeze({
  TRAIN: 'infernet.temporal.train',
  INFER: 'infernet.temporal.infer',
});
export const FAMILIES = Object.freeze(['cfc', 'ltc', 'gru']);
export const RESEARCH_FAMILIES = Object.freeze(['ltc']);
export const ACTION_TYPES = Object.freeze([
  'click',
  'double_click',
  'type',
  'key',
  'scroll',
  'navigate',
  'select',
  'focus',
  'wait',
  'submit',
  'send',
  'post',
  'publish',
  'delete',
  'done',
]);
// Actions with an effect outside the local UI. W3BS asks a person at action time for every one
// of these, in every mode, including Bounded automation.
export const EXTERNAL_WRITE_ACTIONS = Object.freeze([
  'submit',
  'send',
  'post',
  'publish',
  'delete',
]);
export const RUNTIME_PROTOCOL_VERSION = 1;
export const ERROR_CODES = Object.freeze({
  E_BAD_REQUEST: { retriable: false },
  E_SCHEMA: { retriable: false },
  E_HASH_MISMATCH: { retriable: false },
  E_SECRET_DETECTED: { retriable: false },
  E_CONSENT: { retriable: false },
  E_MODEL_HASH: { retriable: false },
  E_NOT_LOADED: { retriable: false },
  E_SESSION_UNKNOWN: { retriable: false },
  E_SESSION_EXPIRED: { retriable: false },
  E_SEQ_DUPLICATE: { retriable: false },
  E_SEQ_LATE: { retriable: false },
  E_SEQ_GAP: { retriable: false },
  E_CAPACITY: { retriable: true },
  E_TIMEOUT: { retriable: true },
  E_CANCELLED: { retriable: false },
  E_RUNTIME_MISSING: { retriable: false },
  E_RUNTIME: { retriable: true },
});

export const isExternalWrite = (action) => !!action && EXTERNAL_WRITE_ACTIONS.includes(action.type);
