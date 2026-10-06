import { useState, useRef, useEffect } from "react";
import {
	Check,
	ChevronRight,
	ExternalLink,
	LoaderCircle,
	SendHorizontal,
	ShieldAlert,
	Sparkles,
	X
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import type { AntigravitySession, AntigravityStatus } from "../hooks/useAntigravitySessions";
import "./AntigravityIsland.css";

const labels: Record<AntigravityStatus, string> = {
	idle: "Ready",
	running: "Working",
	awaiting_approval: "Needs Allow",
	completed: "Finished",
	interrupted: "Interrupted"
};

function StatusIcon({ status }: { status: AntigravityStatus }) {
	if (status === "awaiting_approval") return <ShieldAlert size={14} />;
	if (status === "running") return <LoaderCircle size={14} className="antigravity-spinner" />;
	if (status === "completed") return <Check size={14} />;
	return <span className="antigravity-status-dot" />;
}

export function AntigravityIsland({
	expanded,
	sessions,
	selected,
	select,
	sourceFilter,
	setSourceFilter,
	installed,
	loading,
	error,
	refresh,
	sendPrompt,
	close
}: {
	expanded: boolean;
	sessions: AntigravitySession[];
	selected: AntigravitySession | null;
	select: (id: string) => void;
	sourceFilter: "all" | "IDE" | "Antigravity 2.0";
	setSourceFilter: (filter: "all" | "IDE" | "Antigravity 2.0") => void;
	installed: boolean;
	loading: boolean;
	error: string;
	refresh: () => Promise<void>;
	sendPrompt: (
		prompt: string,
		sessionId?: string | null,
		workspacePath?: string | null,
		source?: string | null
	) => Promise<string>;
	close: () => void;
}) {
	const [busy, setBusy] = useState(false);
	const [actionError, setActionError] = useState("");
	const [promptInput, setPromptInput] = useState("");
	const [isSendingPrompt, setIsSendingPrompt] = useState(false);
	const [promptSuccess, setPromptSuccess] = useState("");
	const inputRef = useRef<HTMLInputElement>(null);

	const running = sessions.filter((s) => s.status === "running").length;
	const approvals = sessions.filter((s) => s.status === "awaiting_approval");
	const pending = selected?.status === "awaiting_approval";

	useEffect(() => {
		if (promptSuccess) {
			const timer = setTimeout(() => setPromptSuccess(""), 3000);
			return () => clearTimeout(timer);
		}
	}, [promptSuccess]);

	const open = async () => {
		if (busy) return;
		setBusy(true);
		setActionError("");
		try {
			await invoke("open_antigravity_chat", {
				id: selected?.id ?? "",
				source: selected?.source ?? "IDE"
			});
		} catch (e) {
			setActionError(String(e));
		} finally {
			setBusy(false);
		}
	};

	const handleSendPrompt = async () => {
		const text = promptInput.trim();
		if (!text || isSendingPrompt) return;
		setIsSendingPrompt(true);
		setActionError("");
		try {
			await sendPrompt(
				text,
				selected?.id,
				selected?.workspacePath,
				selected?.source
			);
			setPromptInput("");
			setPromptSuccess("Prompt sent!");
		} catch (e) {
			setActionError(String(e));
		} finally {
			setIsSendingPrompt(false);
		}
	};

	const handleQuickAllow = async () => {
		setIsSendingPrompt(true);
		setActionError("");
		try {
			// First try focusing
			await open();
			// Also send "Yes" confirmation
			await sendPrompt(
				"Proceed / Yes",
				selected?.id,
				selected?.workspacePath,
				selected?.source
			);
			setPromptSuccess("Approved!");
		} catch (e) {
			setActionError(String(e));
		} finally {
			setIsSendingPrompt(false);
		}
	};

	if (!expanded) {
		return (
			<div className={`antigravity-compact antigravity-${selected?.status ?? "idle"}`}>
				<Sparkles size={15} className="antigravity-compact-icon" />
				<span className="antigravity-compact-title">
					{selected?.title ?? "Antigravity"}
				</span>
				<StatusIcon status={selected?.status ?? "idle"} />
				<span className="antigravity-compact-state">
					{selected ? labels[selected.status] : installed ? "Ready" : "Offline"}
				</span>
				{approvals.length > 0 && !pending && (
					<span
						className="antigravity-pending-badge"
						aria-label={`${approvals.length} tasks need approval`}
					>
						{approvals.length} Allow
					</span>
				)}
			</div>
		);
	}

	return (
		<section className="antigravity-island" aria-label="Antigravity tasks">
			<header className="antigravity-header">
				<Sparkles size={16} color="#a78bfa" />
				<strong>Antigravity</strong>
				<div className="antigravity-filter-tabs">
					<button
						className={`antigravity-filter-btn ${sourceFilter === "all" ? "active" : ""}`}
						onClick={() => setSourceFilter("all")}
					>
						All
					</button>
					<button
						className={`antigravity-filter-btn ${sourceFilter === "IDE" ? "active" : ""}`}
						onClick={() => setSourceFilter("IDE")}
					>
						IDE
					</button>
					<button
						className={`antigravity-filter-btn ${sourceFilter === "Antigravity 2.0" ? "active" : ""}`}
						onClick={() => setSourceFilter("Antigravity 2.0")}
					>
						2.0
					</button>
				</div>
				<span className="antigravity-header-count">
					{approvals.length
						? `${approvals.length} need Allow`
						: running
							? `${running} active`
							: "Agent tasks"}
				</span>
				<button
					className="antigravity-icon-button"
					onClick={close}
					aria-label="Back to status"
				>
					<X size={14} />
				</button>
			</header>

			{sessions.length > 0 && selected ? (
				<>
					<div className="antigravity-session-list" aria-label="Select an agent task">
						{sessions.map((session) => (
							<button
								key={session.id}
								className={`antigravity-session antigravity-${session.status} ${selected.id === session.id ? "selected" : ""}`}
								onClick={() => {
									select(session.id);
									setActionError("");
								}}
								aria-pressed={selected.id === session.id}
							>
								<StatusIcon status={session.status} />
								<span className="antigravity-session-title" title={session.title}>
									{session.title}
								</span>
								<span className="antigravity-badge-source">
									{session.source === "IDE" ? "IDE" : "2.0"}
								</span>
								<span className="antigravity-session-state">
									{labels[session.status]}
								</span>
								<ChevronRight size={12} opacity={0.6} />
							</button>
						))}
					</div>

					<div className={`antigravity-detail antigravity-${selected.status}`}>
						<div className="antigravity-detail-label">
							<StatusIcon status={selected.status} />
							<span>{labels[selected.status]}</span>
							<time title={new Date(selected.updatedAt).toLocaleString()}>
								{new Date(selected.updatedAt).toLocaleTimeString([], {
									hour: "2-digit",
									minute: "2-digit"
								})}
							</time>
						</div>
						<p title={selected.detail}>
							{selected.detail || "Open Antigravity to view this task."}
						</p>
						<div className="antigravity-detail-footer">
							<span title={selected.project}>{selected.project || "Workspace"}</span>
							{pending ? (
								<div className="antigravity-action-group">
									<button
										onClick={() => {
											void handleQuickAllow();
										}}
										disabled={busy || isSendingPrompt}
										className="antigravity-approval-button"
										title="Quick Allow & resume"
									>
										Quick Allow
									</button>
									<button
										onClick={() => {
											void open();
										}}
										disabled={busy}
										className="antigravity-open-button"
									>
										Review <ExternalLink size={11} />
									</button>
								</div>
							) : (
								<button
									onClick={() => {
										void open();
									}}
									disabled={busy}
									className="antigravity-open-button"
								>
									{busy ? "Focusing…" : "Open Chat"}
									<ExternalLink size={12} />
								</button>
							)}
						</div>
					</div>

					{/* Quick Prompt Bar: Send Chat Directly into Antigravity! */}
					<div className="antigravity-prompt-bar">
						<input
							ref={inputRef}
							type="text"
							className="antigravity-prompt-input"
							placeholder={
								selected
									? `Message Antigravity in ${selected.project}...`
									: "Ask Antigravity... (Enter to send)"
							}
							value={promptInput}
							onChange={(e) => setPromptInput(e.target.value)}
							onKeyDown={(e) => {
								if (e.key === "Enter" && !e.shiftKey) {
									e.preventDefault();
									void handleSendPrompt();
								}
							}}
							disabled={isSendingPrompt}
						/>
						<button
							className="antigravity-send-button"
							onClick={() => {
								void handleSendPrompt();
							}}
							disabled={!promptInput.trim() || isSendingPrompt}
							aria-label="Send prompt to Antigravity"
							title="Send prompt to Antigravity"
						>
							{isSendingPrompt ? (
								<LoaderCircle size={14} className="antigravity-spinner" />
							) : (
								<SendHorizontal size={14} />
							)}
						</button>
					</div>
					{promptSuccess && (
						<div className="antigravity-prompt-feedback" role="status">
							<Check size={12} /> {promptSuccess}
						</div>
					)}
				</>
			) : (
				<div className="antigravity-empty">
					<Sparkles size={28} strokeWidth={1.5} />
					<strong>
						{loading
							? "Syncing Antigravity tasks…"
							: installed
								? "Ready for your next prompt"
								: "Antigravity not found"}
					</strong>
					<p>
						{installed
							? "Active chats, running tool tasks, and approval requests from Antigravity IDE and Antigravity 2.0 will appear here automatically."
							: "Install Antigravity IDE or Antigravity 2.0 to monitor agent tasks."}
					</p>
					{installed && !loading && (
						<button
							onClick={() => {
								void open();
							}}
							disabled={busy}
							className="antigravity-open-button"
						>
							{busy ? "Opening…" : "Launch Antigravity IDE"}
						</button>
					)}
				</div>
			)}

			{(error || actionError) && (
				<div className="antigravity-error" role="alert">
					{actionError || error}
					<button
						onClick={() => {
							setActionError("");
							void refresh();
						}}
					>
						Retry
					</button>
				</div>
			)}
		</section>
	);
}
