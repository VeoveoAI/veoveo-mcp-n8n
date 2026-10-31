'use strict';
// The Veoveo Result posts the run's output, the JSON of the items it receives, to the
// run's callback on the server's loopback listener, and passes the items on. A run
// without a callback completes from the server's status check instead.

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

class VeoveoResult {
	constructor() {
		this.description = {
			displayName: 'Veoveo Result',
			name: 'veoveoResult',
			icon: 'fa:flag-checkered',
			group: ['output'],
			version: 1,
			description: "Returns the items it receives as the Veoveo run's output",
			defaults: { name: 'Veoveo Result' },
			inputs: ['main'],
			outputs: ['main'],
			properties: [
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
		const run = runOf(this.getNodeParameter('run', 0));
		if (run && run.callbackUrl) {
			await this.helpers.httpRequest({
				method: 'POST',
				url: run.callbackUrl,
				body: { executionId: String(this.getExecutionId()), output: items.map((item) => item.json) },
				json: true,
			});
		}
		return [items];
	}
}

module.exports = { VeoveoResult };
