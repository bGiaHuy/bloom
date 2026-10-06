import { useState } from "react";
import { Sparkles } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { SettingRow } from "./SettingRow";
import { useAntigravitySessions } from "../hooks/useAntigravitySessions";

export function AntigravitySettings() {
	const agy = useAntigravitySessions();
	const [busy, setBusy] = useState(false);
	const [message, setMessage] = useState("");
	const [error, setError] = useState("");

	const launch = async () => {
		setBusy(true);
		setMessage("");
		setError("");
		try {
			await invoke("open_antigravity_chat", { id: "", source: "IDE" });
			setMessage("Launched / focused Antigravity.");
		} catch (e) {
			setError(String(e));
		} finally {
			setBusy(false);
		}
	};

	const toggle = async () => {
		const value = String(!agy.enabled);
		localStorage.setItem("bloom-antigravity-enabled", value);
		try {
			await invoke("save_setting", { key: "bloom-antigravity-enabled", value });
		} catch {
			setError("Cannot save the Antigravity display setting.");
		}
	};

	const statusDesc = agy.snapshot.installed
		? `Detected: ${agy.snapshot.ideInstalled ? "Antigravity IDE" : ""} ${agy.snapshot.appInstalled ? "• Antigravity 2.0" : ""} (${agy.sessions.length} chats synced)`
		: "Antigravity installation not detected in standard paths.";

	return (
		<>
			<div className="setting-group-label">Antigravity</div>
			<div className="setting-group">
				<SettingRow
					icon={Sparkles}
					label="Antigravity island"
					desc="Antigravity IDE and Antigravity 2.0 tasks, real-time tool state and approvals"
				>
					<label className="toggle-switch">
						<input
							aria-label="Show Antigravity island"
							type="checkbox"
							checked={agy.enabled}
							onChange={() => {
								void toggle();
							}}
						/>
						<span className="slider" />
					</label>
				</SettingRow>
				<SettingRow
					icon={Sparkles}
					label="Status & Launch"
					desc={statusDesc}
					divider={false}
				>
					<button
						className="settings-select"
						disabled={busy || !agy.snapshot.installed}
						onClick={() => {
							void launch();
						}}
					>
						{busy ? "Opening…" : "Focus / Launch"}
					</button>
				</SettingRow>
			</div>
			{(message || error || agy.error) && (
				<p
					role={error || agy.error ? "alert" : "status"}
					style={{ fontSize: 12, lineHeight: 1.5, opacity: 0.8 }}
				>
					{error || agy.error || message}
				</p>
			)}
		</>
	);
}
