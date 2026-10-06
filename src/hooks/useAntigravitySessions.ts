import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsSync } from "./useSettingsSync";

export type AntigravityStatus =
	| "idle"
	| "running"
	| "awaiting_approval"
	| "completed"
	| "interrupted";

export interface AntigravitySession {
	id: string;
	title: string;
	project: string;
	status: AntigravityStatus;
	detail: string;
	updatedAt: number;
	source: "IDE" | "Antigravity 2.0" | string;
	workspacePath?: string | null;
}

export interface AntigravitySnapshot {
	installed: boolean;
	ideInstalled: boolean;
	appInstalled: boolean;
	sessions: AntigravitySession[];
}

export function useAntigravitySessions() {
	const [enabled, setEnabled] = useState(
		() => localStorage.getItem("bloom-antigravity-enabled") !== "false"
	);
	const [snapshot, setSnapshot] = useState<AntigravitySnapshot>({
		installed: false,
		ideInstalled: false,
		appInstalled: false,
		sessions: []
	});
	const [error, setError] = useState("");
	const [loading, setLoading] = useState(true);
	const [selectedId, setSelectedId] = useState<string | null>(null);
	const [sourceFilter, setSourceFilter] = useState<"all" | "IDE" | "Antigravity 2.0">("all");

	const refreshRef = useRef<() => Promise<void>>(async () => {});
	const refresh = useCallback(() => refreshRef.current(), []);
	useSettingsSync({ "bloom-antigravity-enabled": setEnabled });

	useEffect(() => {
		let disposed = false;
		invoke<Record<string, string>>("load_settings")
			.then((settings) => {
				if (!disposed && settings["bloom-antigravity-enabled"] !== undefined) {
					setEnabled(settings["bloom-antigravity-enabled"] !== "false");
				}
			})
			.catch(() => {});
		return () => {
			disposed = true;
		};
	}, []);

	useEffect(() => {
		let disposed = false;
		let inFlight = false;
		const read = async () => {
			if (inFlight || disposed) return;
			inFlight = true;
			try {
				const next = await invoke<AntigravitySnapshot>("get_antigravity_sessions");
				if (!disposed) {
					setSnapshot((previous) =>
						JSON.stringify(previous) === JSON.stringify(next) ? previous : next
					);
					setError("");
				}
			} catch {
				if (!disposed) {
					setError("Cannot read Antigravity status.");
					setSnapshot((previous) => ({ ...previous, sessions: [] }));
				}
			} finally {
				inFlight = false;
				if (!disposed) setLoading(false);
			}
		};
		refreshRef.current = read;
		void read();
		const interval = setInterval(
			() => {
				void read();
			},
			enabled ? 1500 : 8000
		);
		return () => {
			disposed = true;
			clearInterval(interval);
		};
	}, [enabled]);

	const sendPrompt = useCallback(
		async (
			prompt: string,
			sessionId?: string | null,
			workspacePath?: string | null,
			source?: string | null
		) => {
			const res = await invoke<string>("send_antigravity_prompt", {
				prompt,
				sessionId: sessionId ?? null,
				workspacePath: workspacePath ?? null,
				source: source ?? null
			});
			await refresh();
			return res;
		},
		[refresh]
	);

	const allSessions = enabled ? snapshot.sessions : [];
	const filteredSessions =
		sourceFilter === "all"
			? allSessions
			: allSessions.filter((s) => s.source.toLowerCase().includes(sourceFilter.toLowerCase()));

	const selected =
		filteredSessions.find((s) => s.id === selectedId) ?? filteredSessions[0] ?? null;

	return {
		enabled,
		snapshot,
		sessions: filteredSessions,
		allSessions,
		selected,
		select: setSelectedId,
		sourceFilter,
		setSourceFilter,
		sendPrompt,
		error,
		loading,
		refresh
	};
}
