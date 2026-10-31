'use strict';
// The Veoveo Trigger starts a workflow for a Veoveo caller's run. The server calls it on
// loopback at n8n's production webhook `{webhookId}/run`, with a header that only the
// server sets: the hooks never forward the server's headers. It answers at once with the
// execution it started and emits one item: the run's input, and the run it belongs to,
// which the Veoveo Result and Veoveo Tool nodes read. Its input schema, a JSON Schema
// object, describes the input for the overview App's run form; n8n does not check it.

const DISPATCH_HEADER = 'x-veoveo-dispatch';

class VeoveoTrigger {
	constructor() {
		this.description = {
			displayName: 'Veoveo Trigger',
			name: 'veoveoTrigger',
			icon: 'fa:play-circle',
			group: ['trigger'],
			version: 1,
			description: 'Starts the workflow when a Veoveo caller runs it',
			defaults: { name: 'Veoveo Trigger' },
			inputs: [],
			outputs: ['main'],
			webhooks: [
				{
					name: 'default',
					httpMethod: 'POST',
					responseMode: 'onReceived',
					responseData: '={{ String($execution.id) }}',
					path: 'run',
				},
			],
			properties: [
				{
					displayName:
						'A Veoveo caller runs this workflow with the n8n server\'s run tool; the input arrives as the item\'s "input".',
					name: 'notice',
					type: 'notice',
					default: '',
				},
				{
					displayName: 'Input Schema',
					name: 'inputSchema',
					type: 'json',
					default: '{"type": "object", "properties": {}}',
					description:
						'A JSON Schema object of the run\'s input, from which the overview App builds its run form',
				},
			],
		};
	}

	async webhook() {
		const headers = this.getHeaderData();
		if (headers[DISPATCH_HEADER] !== 'run') {
			const response = this.getResponseObject();
			response.status(404).end();
			return { noWebhookResponse: true };
		}
		const body = this.getBodyData() || {};
		const run = {
			operationId: String(body.operationId || ''),
			runToken: String(body.runToken || ''),
			callbackUrl: body.callbackUrl || null,
			bridgeUrl: body.bridgeUrl || null,
		};
		const input = body.input && typeof body.input === 'object' ? body.input : {};
		return { workflowData: [[{ json: { input, veoveo: run } }]] };
	}
}

module.exports = { VeoveoTrigger };
