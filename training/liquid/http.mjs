// Liquid Agent HTTP API, mounted by the local app at /api/v1/training/liquid behind the same
// human session, loopback Host, custom header and Origin checks as the rest of the local API.
// The umbrella PRD's routes map as:
//   POST /training/episodes             -> POST /api/v1/training/liquid/episodes
//   POST /training/datasets             -> POST /api/v1/training/liquid/datasets
//   POST /training/evaluate             -> POST /api/v1/training/liquid/evaluate
//   POST /training/jobs                 -> POST /api/v1/training/liquid/jobs
//   GET  /training/jobs/:id             -> GET  /api/v1/training/liquid/jobs/:id
//   POST /training/models/:id/activate  -> POST /api/v1/training/liquid/models/:id/activate
// (/api/v1/training/datasets already names W3BS Training's own format-0.1 datasets.)
import express from 'express';

const wrap = (fn) => async (req, res) => {
  try {
    const result = await fn(req, res);
    if (result !== undefined) res.json(result);
  } catch (error) {
    res.status(error.status && error.status >= 400 ? error.status : 500).json({
      error: {
        code: error.code || 'REQUEST_FAILED',
        message: error.message,
        ...(error.details ? { details: error.details } : {}),
      },
    });
  }
};

export function liquidRouter(service, { actorFor, onChange = () => {} }) {
  const router = express.Router();
  const json = express.json({ limit: '8mb', strict: true });
  const L = (req) => {
    const svc = typeof service === 'function' ? service(req) : service;
    if (!svc?.liquid)
      throw Object.assign(new Error('The Liquid Agent runs only in the local app.'), {
        code: 'UNAVAILABLE',
        status: 501,
      });
    return svc.liquid;
  };
  const a = (req) => actorFor(req);
  const changed = (result) => {
    onChange('liquid');
    return result;
  };
  const body = (req) => req.body ?? {};

  router.get(
    '/status',
    wrap((req) => L(req).status()),
  );
  router.get(
    '/permissions',
    wrap((req) => L(req).permissions()),
  );
  router.post(
    '/permissions/:name',
    json,
    wrap((req) =>
      L(req)
        .setPermission(req.params.name, body(req).on === true, a(req), { scope: body(req).scope })
        .then(changed),
    ),
  );
  router.post(
    '/kill-switch',
    json,
    wrap((req) =>
      L(req)
        .setKillSwitch(body(req).on !== false, a(req))
        .then(changed),
    ),
  );

  router.post(
    '/demo',
    json,
    wrap((req) =>
      L(req)
        .recordMockDemonstrations({ count: Math.min(Number(body(req).count) || 20, 60) }, a(req))
        .then(changed),
    ),
  );
  router.get(
    '/captures',
    wrap((req) => L(req).listCaptures()),
  );
  router.post(
    '/captures',
    json,
    wrap((req) => L(req).startCapture(body(req), a(req)).then(changed)),
  );
  router.post(
    '/captures/:id/steps',
    json,
    wrap((req) => L(req).captureStep(req.params.id, body(req), a(req))),
  );
  router.post(
    '/captures/:id/pause',
    wrap((req) => L(req).pauseCapture(req.params.id, a(req)).then(changed)),
  );
  router.post(
    '/captures/:id/resume',
    wrap((req) => L(req).resumeCapture(req.params.id, a(req)).then(changed)),
  );
  router.post(
    '/captures/:id/stop',
    json,
    wrap((req) =>
      L(req)
        .stopCapture(req.params.id, a(req), { discard: body(req).discard === true })
        .then(changed),
    ),
  );

  router.get(
    '/episodes',
    wrap((req) => L(req).listEpisodes({ state: req.query.state })),
  );
  router.post(
    '/episodes',
    json,
    wrap((req) => L(req).addEpisodes(body(req).episodes, a(req)).then(changed)),
  );
  router.post(
    '/episodes/import',
    json,
    wrap((req) =>
      L(req)
        .importTrainingEpisode(body(req).episode_id, { task: body(req).task }, a(req))
        .then(changed),
    ),
  );
  router.post(
    '/episodes/dedupe',
    wrap((req) => L(req).dedupe(a(req)).then(changed)),
  );
  router.get(
    '/episodes/:id',
    wrap((req) => L(req).getEpisode(req.params.id)),
  );
  router.post(
    '/episodes/:id/review',
    json,
    wrap((req) => L(req).review(req.params.id, body(req), a(req)).then(changed)),
  );
  router.post(
    '/episodes/:id/redact',
    json,
    wrap((req) => L(req).redact(req.params.id, body(req), a(req)).then(changed)),
  );
  router.post(
    '/episodes/:id/unredact',
    json,
    wrap((req) => L(req).unredact(req.params.id, body(req).redaction_id, a(req)).then(changed)),
  );
  router.post(
    '/episodes/:id/discard',
    wrap((req) => L(req).discardEpisode(req.params.id, a(req)).then(changed)),
  );
  router.post(
    '/episodes/:id/accept',
    wrap((req) => L(req).acceptCorrection(req.params.id, a(req)).then(changed)),
  );
  router.delete(
    '/episodes/:id',
    wrap((req) => L(req).deleteEpisode(req.params.id, a(req)).then(changed)),
  );

  router.get(
    '/datasets',
    wrap((req) => L(req).listDatasets()),
  );
  router.post(
    '/datasets',
    json,
    wrap((req) => L(req).exportDataset(body(req), a(req)).then(changed)),
  );
  router.get(
    '/datasets/:id',
    wrap((req) => L(req).getDataset(req.params.id)),
  );

  router.post(
    '/jobs/preview',
    json,
    wrap((req) => L(req).previewRemoteTrain(body(req), a(req))),
  );
  router.get(
    '/jobs',
    wrap((req) => L(req).listJobs()),
  );
  router.post(
    '/jobs',
    json,
    wrap((req) => L(req).train(body(req), a(req)).then(changed)),
  );
  router.get(
    '/jobs/:id',
    wrap((req) => L(req).getJob(req.params.id)),
  );

  router.get(
    '/models',
    wrap((req) => L(req).listModels()),
  );
  router.post(
    '/evaluate',
    json,
    wrap((req) => L(req).evaluate(body(req), a(req)).then(changed)),
  );
  router.get(
    '/evaluations',
    wrap((req) => L(req).listEvaluations(req.query.model_id)),
  );
  router.post(
    '/models/:id/activate',
    json,
    wrap((req) =>
      L(req)
        .activate(req.params.id, { tasks: body(req).tasks }, a(req))
        .then(changed),
    ),
  );
  router.post(
    '/tasks/:task/deactivate',
    wrap((req) => L(req).deactivate(req.params.task, a(req)).then(changed)),
  );
  router.get(
    '/tasks/:task/capability',
    wrap((req) => L(req).capability(req.params.task)),
  );
  router.post(
    '/tasks/:task/mode',
    json,
    wrap((req) =>
      L(req)
        .setMode(req.params.task, body(req).mode, a(req), { bounds: body(req).bounds })
        .then(changed),
    ),
  );

  router.get(
    '/runs',
    wrap((req) => L(req).listRuns()),
  );
  router.post(
    '/runs',
    json,
    // Fault injection is for tests only and never reachable over HTTP.
    wrap((req) => {
      const { task, seed, variant, account } = body(req);
      return L(req).startRun({ task, seed, variant, account }, a(req)).then(changed);
    }),
  );
  router.get(
    '/runs/:id',
    wrap((req) => L(req).getRun(req.params.id)),
  );
  router.get(
    '/asks',
    wrap((req) => L(req).listAsks({ state: req.query.state ?? 'pending' })),
  );
  router.post(
    '/asks/:id',
    json,
    wrap((req) => L(req).answerAsk(req.params.id, body(req), a(req)).then(changed)),
  );

  router.get(
    '/audit',
    wrap((req) => L(req).auditLog({ kind: req.query.kind })),
  );
  return router;
}
