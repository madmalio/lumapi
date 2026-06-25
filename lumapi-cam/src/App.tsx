import { useState } from "react";

export default function App() {
  const [isRecording, setIsRecording] = useState(false);

  return (
    // We force the root background to be completely transparent
    <div className="relative w-screen h-screen bg-transparent text-white overflow-hidden">
      {/* THE TAILWIND HUD OVERLAY LAYER */}
      <div className="absolute inset-0 z-10 flex flex-col justify-between p-6 pointer-events-none">
        {/* TOP STATUS BAR */}
        <div className="flex justify-between items-center pointer-events-auto">
          <div className="text-xl font-bold tracking-wider drop-shadow-[0_2px_4px_rgba(0,0,0,0.8)]">
            LUMAPI-CAM
          </div>
          <div className="bg-emerald-500/80 text-white px-3 py-1 rounded text-sm font-medium border border-emerald-400 shadow-md">
            100% BAT
          </div>
        </div>

        {/* CENTER FRAME GUIDE */}
        <div className="flex-1 flex items-center justify-center">
          <div className="w-16 h-16 border-2 border-white/40 border-dashed rounded-full" />
        </div>

        {/* BOTTOM CAMERA CONTROLS */}
        <div className="flex justify-between items-center pointer-events-auto px-10">
          <div className="w-12" />

          {/* Giant Record Button */}
          <button
            onClick={() => setIsRecording(!isRecording)}
            className="w-20 h-20 rounded-full border-4 border-white flex items-center justify-center p-1 bg-black/60 backdrop-blur-md shadow-2xl transition-all active:scale-95 cursor-pointer"
          >
            <div
              className={`w-full h-full rounded-full transition-all duration-300 ${isRecording ? "bg-red-500 scale-50 rounded-sm" : "bg-red-600"}`}
            />
          </button>

          <div className="text-sm font-semibold tracking-wide bg-black/60 backdrop-blur-md px-3 py-2 rounded-md border border-white/20 shadow-md">
            1080P 60
          </div>
        </div>
      </div>
    </div>
  );
}
