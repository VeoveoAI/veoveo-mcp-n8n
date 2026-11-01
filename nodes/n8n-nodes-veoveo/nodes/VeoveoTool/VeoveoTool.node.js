'use strict';
// The Veoveo Tool calls a tool of another Veoveo server for the run's caller, through the
// server's bridge on loopback, with the run's token. The bridge admits only the tools in
// its table and only a running run's token; this node holds no other logic. In a person's
// own Computer no run starts the workflow: veoveo-n8n-computer names its own bridge and
// token in n8n's environment, and the call acts as the Computer's owner.

function runOf(value) {
	if (typeof value === 'string') {
		try {
			return JSON.parse(value);
		} catch {
			return null;
		}
	}
	return value && typeof value === 'object' ? value : null;
}

// The Computer's bridge, when n8n runs in a person's own Computer.
function computerRun() {
	const bridgeUrl = process.env.VEOVEO_BRIDGE_URL;
	const runToken = process.env.VEOVEO_BRIDGE_TOKEN;
	return bridgeUrl && runToken ? { runToken, bridgeUrl } : null;
}

function objectOf(value) {
	if (typeof value === 'string') {
		const parsed = value.trim() === '' ? {} : JSON.parse(value);
		return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {};
	}
	return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
}

class VeoveoTool {
	constructor() {
		this.description = {
			displayName: 'Veoveo Tool',
			name: 'veoveoTool',
			icon: 'fa:plug',
			group: ['transform'],
			version: 1,
			description: "Calls another Veoveo server's tool for the run's caller",
			defaults: { name: 'Veoveo Tool' },
			inputs: ['main'],
			outputs: ['main'],
			properties: [
				{
					displayName: 'Tool',
					name: 'tool',
					type: 'string',
					default: '',
					required: true,
					placeholder: 'time__resolve_time',
					description: "The tool's name through the Hub, {server}__{tool}",
				},
				{
					displayName: 'Arguments',
					name: 'arguments',
					type: 'json',
					default: '{}',
					description: "The tool's arguments, a JSON object",
				},
				{
					displayName: 'Run',
					name: 'run',
					type: 'json',
					default: '={{ $("Veoveo Trigger").first().json.veoveo }}',
					description: 'The run, as the Veoveo Trigger emitted it',
				},
			],
		};
	}

	async execute() {
		const items = this.getInputData();
		const output = [];
		for (let index = 0; index < items.length; index++) {
			let run = null;
			try {
				run = runOf(this.getNodeParameter('run', index));
			} catch {
				// A workflow without a Veoveo Trigger names no run.
			}
			if (!run || !run.runToken || !run.bridgeUrl) {
				run = computerRun();
			}
			if (!run) {
				throw new Error('The Veoveo Tool runs only in a run that a Veoveo Trigger started, or in your Computer');
			}
			const response = await this.helpers.httpRequest({
				method: 'POST',
				url: run.bridgeUrl,
				body: {
					runToken: run.runToken,
					tool: this.getNodeParameter('tool', index),
					arguments: objectOf(this.getNodeParameter('arguments', index)),
				},
				json: true,
				returnFullResponse: true,
				ignoreHttpStatusErrors: true,
			});
			const body = response.body || {};
			if (response.statusCode !== 200 || body.isError) {
				const reason = body.error || JSON.stringify(body.result || body);
				if (this.continueOnFail()) {
					output.push({ json: { error: reason }, pairedItem: { item: index } });
					continue;
				}
				throw new Error(`The bridge refused or the tool failed: ${reason}`);
			}
			output.push({ json: body.result, pairedItem: { item: index } });
		}
		return [output];
	}
}

module.exports = { VeoveoTool };
